use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, TimeZone as _, Utc};
use rayon::prelude::*;
use regex::Regex;
use serde_json::{Map, Value, json};
use walkdir::WalkDir;

use super::SessionProvider;
use crate::{
    AppType, MessageType, Session, SessionDetail, SessionMessage,
    model::{SessionKind, ToolOutput},
};

#[derive(Debug, Clone)]
pub struct CodeBuddyProvider {
    root: PathBuf,
}

#[derive(Debug, Clone)]
struct SessionFile {
    path: PathBuf,
    project_cwd: PathBuf,
    id: String,
    internal_id: Option<String>,
    parent_id: Option<String>,
    agent_type: Option<String>,
    kind: SessionKind,
    size: u64,
    created_at: i64,
    updated_at: i64,
}

impl Default for CodeBuddyProvider {
    fn default() -> Self {
        Self {
            root: dirs::home_dir().unwrap_or_default().join(".codebuddy"),
        }
    }
}

impl CodeBuddyProvider {
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }
    fn projects_path(&self) -> PathBuf {
        self.root.join("projects")
    }

    fn values(path: &Path) -> Vec<Value> {
        fs::read_to_string(path)
            .ok()
            .map(|source| {
                source
                    .lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn summary_values(path: &Path) -> Vec<Value> {
        const SUMMARY_PREFIX_BYTES: u64 = 256 * 1024;
        let mut bytes = Vec::with_capacity(SUMMARY_PREFIX_BYTES as usize);
        if fs::File::open(path)
            .and_then(|file| {
                file.take(SUMMARY_PREFIX_BYTES)
                    .read_to_end(&mut bytes)
                    .map(|_| ())
            })
            .is_err()
        {
            return Vec::new();
        }
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn string(value: Option<&Value>) -> Option<String> {
        value
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    }

    fn timestamp_ms(value: Option<&Value>) -> Option<i64> {
        match value {
            Some(Value::Number(number)) => number.as_i64(),
            Some(Value::String(value)) => DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|time| time.timestamp_millis()),
            _ => None,
        }
    }

    fn iso_time(value: Option<&Value>, fallback: i64) -> String {
        if let Some(Value::String(value)) = value {
            return value.clone();
        }
        Utc.timestamp_millis_opt(Self::timestamp_ms(value).unwrap_or(fallback))
            .single()
            .unwrap_or_else(Utc::now)
            .to_rfc3339()
    }

    fn content_items(value: Option<&Value>) -> &[Value] {
        value
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn text(value: Option<&Value>, accepted: &[&str]) -> String {
        match value {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| {
                    let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
                    (accepted.is_empty() || accepted.contains(&kind))
                        .then(|| item.get("text").and_then(Value::as_str))
                        .flatten()
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        }
    }

    fn message_text(record: &Value) -> String {
        let role = record.get("role").and_then(Value::as_str);
        let accepted: &[&str] = if role == Some("assistant") {
            &["output_text", "text"]
        } else {
            &["input_text", "text", "output_text"]
        };
        let direct = Self::text(record.get("content"), accepted);
        if direct.is_empty() {
            Self::text(record.pointer("/message/content"), &[])
        } else {
            direct
        }
    }

    fn reasoning_text(record: &Value) -> String {
        let raw = Self::text(
            record.get("rawContent"),
            &["reasoning_text", "text", "input_text"],
        );
        if raw.is_empty() {
            Self::text(
                record.get("content"),
                &["reasoning_text", "text", "input_text"],
            )
        } else {
            raw
        }
    }

    fn clean_user_text(text: &str) -> String {
        let patterns = [
            r#"(?is)<system-reminder\b[^>]*data-role\s*=\s*["']?command-caveat["']?[^>]*>.*?</system-reminder\s*>"#,
            r"(?is)<system-reminder>.*?</system-reminder\s*>",
            r"(?is)<local-command-stdout\b[^>]*>.*?</local-command-stdout\s*>",
            r"(?is)<local-command-stderr\b[^>]*>.*?</local-command-stderr\s*>",
            r"(?is)<command-name\b[^>]*>.*?</command-name\s*>",
        ];
        let cleaned = patterns.iter().fold(text.to_owned(), |value, pattern| {
            Regex::new(pattern)
                .map(|regex| regex.replace_all(&value, "").into_owned())
                .unwrap_or(value)
        });
        Regex::new(r"\n{3,}")
            .map(|regex| regex.replace_all(&cleaned, "\n\n").trim().to_owned())
            .unwrap_or(cleaned)
    }

    fn tool_input(value: Option<&Value>) -> Map<String, Value> {
        match value {
            Some(Value::Object(map)) => map.clone(),
            Some(Value::String(text)) => serde_json::from_str(text)
                .unwrap_or_else(|_| Map::from_iter([("arguments".into(), json!(text))])),
            Some(value) if !value.is_null() => Map::from_iter([("value".into(), value.clone())]),
            _ => Map::new(),
        }
    }

    fn stringify(value: Option<&Value>) -> String {
        match value {
            Some(Value::String(text)) => text.clone(),
            Some(value) if value.get("text").and_then(Value::as_str).is_some() => value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            Some(value) if value.get("content").is_some() => Self::text(value.get("content"), &[]),
            Some(value) => serde_json::to_string_pretty(value).unwrap_or_default(),
            None => String::new(),
        }
    }

    fn child_session_id(value: Option<&Value>) -> Option<String> {
        let value = value?;
        fn structured(value: &Value, depth: usize) -> Option<String> {
            if depth > 8 {
                return None;
            }
            let object = value.as_object()?;
            for key in [
                "childSessionId",
                "subAgentSessionId",
                "sessionId",
                "agentId",
            ] {
                if let Some(id) = CodeBuddyProvider::string(object.get(key)) {
                    return Some(id);
                }
            }
            object
                .values()
                .find_map(|child| structured(child, depth + 1))
        }
        if let Some(id) = structured(value, 0) {
            return Some(id);
        }
        let text = Self::stringify(Some(value));
        if let Some(raw) = value.as_str()
            && let Ok(nested) = serde_json::from_str::<Value>(raw)
            && nested != *value
            && let Some(id) = structured(&nested, 0)
        {
            return Some(id);
        }
        Regex::new(r"(?i)\bagent-[a-z0-9_-]+\b").ok()?.find_iter(&text).last().map(|value| value.as_str().to_owned())
            .or_else(|| Regex::new(r#"(?i)(?:childSessionId|subAgentSessionId|sessionId|session)["']?\s*[:=]\s*["']?([a-f0-9-]{36})"#).ok()?.captures(&text)?.get(1).map(|value| value.as_str().to_owned()))
    }

    fn structured_child_id(record: &Value) -> Option<String> {
        Self::string(record.pointer("/providerData/toolResult/subAgent/sessionId"))
    }

    fn mime(path: &Path) -> Option<&'static str> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            "svg" => Some("image/svg+xml"),
            "bmp" => Some("image/bmp"),
            "ico" => Some("image/x-icon"),
            "avif" => Some("image/avif"),
            _ => None,
        }
    }

    fn image_markdown(&self, item: &Value) -> Option<String> {
        const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
        let path = PathBuf::from(item.get("blob_path")?.as_str()?);
        let name = path.file_name()?.to_string_lossy();
        let canonical = path.canonicalize().ok()?;
        let root = self.root.canonicalize().ok()?;
        if !canonical.starts_with(root) || fs::metadata(&canonical).ok()?.len() > MAX_IMAGE_BYTES {
            return Some(format!("📎 {name}"));
        }
        let mime = item
            .get("mime")
            .and_then(Value::as_str)
            .or_else(|| Self::mime(&canonical))?;
        Some(format!(
            "![{name}](data:{mime};base64,{})",
            STANDARD.encode(fs::read(canonical).ok()?)
        ))
    }

    fn normalize(&self, records: &[Value], fallback: i64) -> Vec<SessionMessage> {
        let mut messages = Vec::new();
        let mut pending_agents: HashMap<String, Map<String, Value>> = HashMap::new();
        let mut current_model = None;
        let mut pending_reasoning = String::new();

        for record in records {
            current_model = Self::string(record.pointer("/providerData/model")).or(current_model);
            let timestamp = Self::iso_time(record.get("timestamp"), fallback);
            let kind = record
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if kind == "reasoning" {
                let value = Self::reasoning_text(record);
                if !value.is_empty() {
                    pending_reasoning = [pending_reasoning, value]
                        .into_iter()
                        .filter(|value| !value.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n\n")
                }
                continue;
            }

            if kind == "message"
                && matches!(
                    record.get("role").and_then(Value::as_str),
                    Some("user" | "system")
                )
            {
                if !pending_reasoning.is_empty() {
                    let mut pending =
                        SessionMessage::text(MessageType::Assistant, timestamp.clone(), "");
                    pending.content = None;
                    pending.reasoning_content = Some(std::mem::take(&mut pending_reasoning));
                    pending.model = current_model.clone();
                    messages.push(pending);
                }
                let role = record.get("role").and_then(Value::as_str).unwrap_or("user");
                let mut text = Self::message_text(record);
                if role == "user" {
                    text = Self::clean_user_text(&text);
                    let images = Self::content_items(record.get("content"))
                        .iter()
                        .filter(|item| {
                            item.get("type").and_then(Value::as_str) == Some("image_blob_ref")
                        })
                        .filter_map(|item| self.image_markdown(item))
                        .collect::<Vec<_>>();
                    text = std::iter::once(text)
                        .chain(images)
                        .filter(|value| !value.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                }
                if text.is_empty() {
                    continue;
                }
                let mut message = SessionMessage::text(
                    if role == "system" {
                        MessageType::System
                    } else {
                        MessageType::User
                    },
                    timestamp,
                    text,
                );
                if let Some(status) = Self::string(record.get("status")) {
                    message.metadata.insert("subtype".into(), json!(status));
                }
                message.model = current_model.clone();
                messages.push(message);
                continue;
            }

            if (kind == "message"
                && record.get("role").and_then(Value::as_str) == Some("assistant"))
                || kind == "assistant"
            {
                let text = Self::message_text(record);
                if !text.is_empty() || !pending_reasoning.is_empty() {
                    let mut message = SessionMessage::text(MessageType::Assistant, timestamp, text);
                    message.reasoning_content = (!pending_reasoning.is_empty())
                        .then(|| std::mem::take(&mut pending_reasoning));
                    if let Some(status) = Self::string(record.get("status")) {
                        message.metadata.insert("subtype".into(), json!(status));
                    }
                    message.model = current_model.clone();
                    messages.push(message);
                }
                continue;
            }

            if kind == "function_call" {
                let name = Self::string(record.get("name")).unwrap_or_else(|| "tool".into());
                let call_id = Self::string(record.get("callId"))
                    .or_else(|| Self::string(record.get("id")))
                    .unwrap_or_else(|| format!("call_{}", messages.len()));
                let input = Self::tool_input(record.get("arguments"));
                let mut message = SessionMessage::text(MessageType::ToolUse, timestamp, "");
                message.content = None;
                message.tool_name = Some(name.clone());
                message.tool_input = Some(input.clone());
                message.call_id = Some(call_id.clone());
                message.reasoning_content =
                    (!pending_reasoning.is_empty()).then(|| std::mem::take(&mut pending_reasoning));
                message.model = current_model.clone();
                if name.to_ascii_lowercase().contains("agent") {
                    pending_agents.insert(call_id, input);
                }
                messages.push(message);
                continue;
            }

            if kind == "function_call_result" {
                if !pending_reasoning.is_empty() {
                    let mut pending =
                        SessionMessage::text(MessageType::Assistant, timestamp.clone(), "");
                    pending.content = None;
                    pending.reasoning_content = Some(std::mem::take(&mut pending_reasoning));
                    pending.model = current_model.clone();
                    messages.push(pending);
                }
                let name = Self::string(record.get("name")).unwrap_or_else(|| "tool".into());
                let call_id = Self::string(record.get("callId"));
                let output = Self::stringify(record.get("output"));
                let input = call_id.as_ref().and_then(|id| pending_agents.remove(id));
                let input_value = input.clone().map(Value::Object);
                let child_id = (name.to_ascii_lowercase().contains("agent") || input.is_some())
                    .then(|| {
                        Self::structured_child_id(record)
                            .or_else(|| Self::child_session_id(record.get("output")))
                            .or_else(|| Self::child_session_id(input_value.as_ref()))
                    })
                    .flatten();
                let mut message = SessionMessage::text(
                    MessageType::ToolResult,
                    timestamp,
                    output.chars().take(300).collect::<String>(),
                );
                message.tool_name = Some(name);
                message.tool_output = Some(ToolOutput {
                    output: Some(output),
                    preview: None,
                    truncated: false,
                    extra: Map::new(),
                });
                message.call_id = call_id;
                if let Some(status) = Self::string(record.get("status")) {
                    message.metadata.insert("subtype".into(), json!(status));
                }
                if let Some(child_id) = child_id {
                    message.sub_agent_session_id = Some(child_id.clone());
                    message
                        .metadata
                        .insert("childSessionId".into(), json!(child_id));
                    message
                        .metadata
                        .insert("childSessionAppType".into(), json!("codebuddy"));
                }
                message.model = current_model.clone();
                messages.push(message);
            }
        }
        if !pending_reasoning.is_empty() {
            let mut pending =
                SessionMessage::text(MessageType::Assistant, Self::iso_time(None, fallback), "");
            pending.content = None;
            pending.reasoning_content = Some(pending_reasoning);
            pending.model = current_model;
            messages.push(pending);
        }
        messages
    }

    fn file_times(path: &Path) -> (i64, i64, u64) {
        let Ok(metadata) = fs::metadata(path) else {
            return (0, 0, 0);
        };
        let millis = |time: std::io::Result<std::time::SystemTime>| {
            time.ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_millis() as i64)
                .unwrap_or(0)
        };
        (
            millis(metadata.created()),
            millis(metadata.modified()),
            metadata.len(),
        )
    }

    fn decode_project(name: &str) -> PathBuf {
        let parts = name.split('-').collect::<Vec<_>>();
        if name.starts_with("Users-") && parts.len() >= 2 {
            return PathBuf::from(format!("/Users/{}/{}", parts[1], parts[2..].join("/")));
        }
        PathBuf::from(format!("/{}", name.replace('-', "/")))
    }

    fn discover(&self) -> Vec<SessionFile> {
        let mut files = Vec::new();
        let Ok(projects) = fs::read_dir(self.projects_path()) else {
            return files;
        };
        for project in projects
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        {
            let project_root = project.path();
            let decoded = Self::decode_project(&project.file_name().to_string_lossy());
            for entry in WalkDir::new(&project_root)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry.file_type().is_file()
                        && entry.path().extension().is_some_and(|ext| ext == "jsonl")
                })
            {
                let path = entry.path().to_path_buf();
                let (created_at, updated_at, size) = Self::file_times(&path);
                let records = Self::summary_values(&path);
                let identity = records
                    .iter()
                    .take(64)
                    .find_map(|value| Self::string(value.get("cwd")))
                    .map(PathBuf::from)
                    .unwrap_or_else(|| decoded.clone());
                let internal_id = records
                    .iter()
                    .take(64)
                    .find_map(|value| Self::string(value.get("sessionId")));
                let agent_type = records.iter().take(64).find_map(|value| {
                    (value
                        .pointer("/providerData/isSubAgent")
                        .and_then(Value::as_bool)
                        == Some(true))
                    .then(|| Self::string(value.pointer("/providerData/agent")))
                    .flatten()
                });
                let relative = path.strip_prefix(&project_root).unwrap_or(&path);
                let components = relative
                    .components()
                    .filter_map(|part| match part {
                        Component::Normal(value) => value.to_str().map(str::to_owned),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let subagent_index = components.iter().position(|part| part == "subagents");
                let kind = if subagent_index.is_some() {
                    SessionKind::Subagent
                } else {
                    SessionKind::Main
                };
                let parent_id = subagent_index
                    .and_then(|index| index.checked_sub(1))
                    .and_then(|index| components.get(index))
                    .cloned();
                let id = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                files.push(SessionFile {
                    path,
                    project_cwd: identity,
                    id,
                    internal_id,
                    parent_id,
                    agent_type,
                    kind,
                    size,
                    created_at,
                    updated_at,
                });
            }
        }
        let aliases = files
            .iter()
            .flat_map(|file| {
                std::iter::once((file.id.clone(), file.id.clone()))
                    .chain(file.internal_id.clone().map(|id| (id, file.id.clone())))
            })
            .collect::<HashMap<_, _>>();
        for file in &mut files {
            if let Some(parent) = &file.parent_id {
                file.parent_id = Some(
                    aliases
                        .get(parent)
                        .cloned()
                        .unwrap_or_else(|| parent.clone()),
                )
            }
        }
        files
    }

    fn active_titles(&self) -> HashMap<String, (String, Option<i64>)> {
        let mut active = HashMap::new();
        let Ok(entries) = fs::read_dir(self.root.join("sessions")) else {
            return active;
        };
        for entry in entries
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        {
            let Ok(source) = fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&source) else {
                continue;
            };
            let Some(id) = Self::string(value.get("sessionId")) else {
                continue;
            };
            let title = Self::string(value.pointer("/meta/currentTopic")).unwrap_or_default();
            let updated = Self::timestamp_ms(value.get("updatedAt"))
                .or_else(|| Self::timestamp_ms(value.get("lastHeartbeat")));
            active.insert(id, (title, updated));
        }
        active
    }

    fn previews(messages: &[SessionMessage]) -> (String, String) {
        let clean = |message: &SessionMessage| {
            message
                .content
                .as_deref()
                .unwrap_or_default()
                .lines()
                .filter(|line| {
                    !line.trim_start().starts_with("![") && !line.trim_start().starts_with("📎")
                })
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .chars()
                .take(100)
                .collect::<String>()
        };
        let first = messages
            .iter()
            .find(|message| message.message_type == MessageType::User)
            .map(&clean)
            .unwrap_or_default();
        let last = messages
            .iter()
            .rev()
            .find(|message| message.message_type == MessageType::User)
            .map(clean)
            .unwrap_or_else(|| first.clone());
        (first, last)
    }

    fn detail_for(&self, file: &SessionFile) -> Option<SessionDetail> {
        if file.size == 0 {
            return None;
        }
        let records = Self::values(&file.path);
        let messages = self.normalize(&records, file.updated_at);
        if messages.is_empty() {
            return None;
        }
        let (first, last) = Self::previews(&messages);
        let created_at = records
            .first()
            .and_then(|value| Self::timestamp_ms(value.get("timestamp")))
            .unwrap_or(file.created_at);
        let updated_at = records
            .last()
            .and_then(|value| Self::timestamp_ms(value.get("timestamp")))
            .unwrap_or(file.updated_at);
        Some(SessionDetail {
            session: Session {
                id: file.id.clone(),
                app_type: AppType::CodeBuddy,
                file_name: if first.is_empty() {
                    file.id.clone()
                } else {
                    first.clone()
                },
                file_path: file.path.clone(),
                created_at,
                updated_at,
                message_count: messages.len(),
                first_message: first,
                last_message: last,
                directory: Some(file.project_cwd.clone()),
                uuid: file.internal_id.clone().or_else(|| Some(file.id.clone())),
                kind: file.kind,
                parent_session_id: file.parent_id.clone(),
                agent_type: file.agent_type.clone(),
            },
            messages,
        })
    }

    fn summary_for(&self, file: &SessionFile) -> Option<Session> {
        if file.size == 0 {
            return None;
        }
        let records = Self::summary_values(&file.path);
        let messages = self.normalize(&records, file.updated_at);
        let (first, last) = Self::previews(&messages);
        let created_at = records
            .first()
            .and_then(|value| Self::timestamp_ms(value.get("timestamp")))
            .unwrap_or(file.created_at);
        Some(Session {
            id: file.id.clone(),
            app_type: AppType::CodeBuddy,
            file_name: if first.is_empty() {
                file.id.clone()
            } else {
                first.clone()
            },
            file_path: file.path.clone(),
            created_at,
            updated_at: file.updated_at,
            message_count: messages.len(),
            first_message: first,
            last_message: last,
            directory: Some(file.project_cwd.clone()),
            uuid: file.internal_id.clone().or_else(|| Some(file.id.clone())),
            kind: file.kind,
            parent_session_id: file.parent_id.clone(),
            agent_type: file.agent_type.clone(),
        })
    }
}

impl SessionProvider for CodeBuddyProvider {
    fn app_type(&self) -> AppType {
        AppType::CodeBuddy
    }
    fn is_available(&self) -> bool {
        self.projects_path().exists()
    }

    fn sessions(&self) -> Result<Vec<Session>> {
        let active = self.active_titles();
        let mut by_id: HashMap<String, (Session, u64)> = HashMap::new();
        let summaries = self
            .discover()
            .into_par_iter()
            .filter_map(|file| self.summary_for(&file).map(|session| (file, session)))
            .collect::<Vec<_>>();
        for (file, mut session) in summaries {
            let active_value = active
                .get(&file.id)
                .or_else(|| file.internal_id.as_ref().and_then(|id| active.get(id)));
            if let Some((title, updated)) = active_value {
                if !title.is_empty() {
                    session.file_name = title.clone();
                    session.last_message = title.clone();
                }
                if let Some(updated) = updated {
                    session.updated_at = *updated;
                }
            }
            match by_id.get(&session.id) {
                Some((_, size)) if *size >= file.size => {}
                _ => {
                    by_id.insert(session.id.clone(), (session, file.size));
                }
            }
        }
        let mut sessions = by_id.into_values().map(|value| value.0).collect::<Vec<_>>();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        Ok(sessions)
    }

    fn session_detail(&self, session_id: &str) -> Result<Option<SessionDetail>> {
        let files = self.discover();
        Ok(files
            .iter()
            .filter(|file| file.id == session_id || file.internal_id.as_deref() == Some(session_id))
            .max_by_key(|file| file.size)
            .and_then(|file| self.detail_for(file)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_codebuddy_control_blocks() {
        let source = "hello<system-reminder>hidden</system-reminder>\n\n\nworld";
        assert_eq!(CodeBuddyProvider::clean_user_text(source), "hello\n\nworld");
    }

    #[test]
    fn decodes_project_directories() {
        assert_eq!(
            CodeBuddyProvider::decode_project("Users-krabswang-Personal-demo"),
            PathBuf::from("/Users/krabswang/Personal/demo")
        );
    }

    #[test]
    fn child_session_search_does_not_reparse_plain_json_values() {
        assert_eq!(CodeBuddyProvider::child_session_id(Some(&json!({}))), None);
        assert_eq!(
            CodeBuddyProvider::child_session_id(Some(&json!({
                "result": { "subAgentSessionId": "agent-child-1" }
            }))),
            Some("agent-child-1".into())
        );
    }
}
