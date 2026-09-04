use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use walkdir::WalkDir;

use crate::{
    AppType, MessageType, Session, SessionDetail, SessionMessage,
    model::{SessionKind, ToolOutput},
};

use super::SessionProvider;

#[derive(Debug, Clone)]
pub struct CodexProvider {
    root: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
struct IndexEntry {
    id: String,
    thread_name: Option<String>,
    updated_at: Option<String>,
}

impl Default for CodexProvider {
    fn default() -> Self {
        Self {
            root: dirs::home_dir().unwrap_or_default().join(".codex"),
        }
    }
}

impl CodexProvider {
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }
    fn sessions_path(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn read_jsonl(path: &Path) -> Result<Vec<Value>> {
        let source =
            fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Ok(source
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect())
    }

    fn read_summary_prefix(path: &Path) -> Option<String> {
        const SUMMARY_PREFIX_BYTES: u64 = 256 * 1024;
        let mut bytes = Vec::with_capacity(SUMMARY_PREFIX_BYTES as usize);
        fs::File::open(path)
            .ok()?
            .take(SUMMARY_PREFIX_BYTES)
            .read_to_end(&mut bytes)
            .ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn load_index(&self) -> HashMap<String, IndexEntry> {
        let Ok(source) = fs::read_to_string(self.root.join("session_index.jsonl")) else {
            return HashMap::new();
        };
        source
            .lines()
            .filter_map(|line| serde_json::from_str::<IndexEntry>(line).ok())
            .map(|entry| (entry.id.clone(), entry))
            .collect()
    }

    fn session_files(&self) -> Vec<PathBuf> {
        WalkDir::new(self.sessions_path())
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_type().is_file()
                    && entry.path().extension().is_some_and(|ext| ext == "jsonl")
            })
            .map(|entry| entry.into_path())
            .collect()
    }

    fn metadata(records: &[Value]) -> Map<String, Value> {
        records
            .iter()
            .find_map(|record| {
                (record.get("type").and_then(Value::as_str) == Some("session_meta"))
                    .then(|| record.get("payload")?.as_object().cloned())?
            })
            .unwrap_or_default()
    }

    fn is_review(records: &[Value]) -> bool {
        records.iter().any(|record| {
            record.get("type").and_then(Value::as_str) == Some("turn_context")
                && record.pointer("/payload/model").and_then(Value::as_str)
                    == Some("codex-auto-review")
        })
    }

    fn file_id(path: &Path) -> Option<String> {
        let regex = Regex::new(
            r"(?i)([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.jsonl$",
        )
        .ok()?;
        regex
            .captures(path.file_name()?.to_str()?)?
            .get(1)
            .map(|value| value.as_str().to_owned())
    }

    fn value_text(value: Option<&Value>) -> String {
        match value {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n"),
            _ => String::new(),
        }
    }

    fn reasoning_text(payload: &Map<String, Value>) -> String {
        if let Some(text) = payload.get("content").and_then(Value::as_str) {
            return text.to_owned();
        }
        payload
            .get("summary")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        item.as_str()
                            .or_else(|| item.get("text").and_then(Value::as_str))
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .unwrap_or_default()
    }

    fn normalize_user(content: &str) -> String {
        let trimmed = content.trim();
        let hidden_prefixes = [
            "# AGENTS.md instructions for",
            "<skill>",
            "<skill ",
            "The following is the Codex agent history",
            "<environment_context>",
            "<permissions instructions>",
            "<app-context>",
            "<collaboration_mode>",
            "<skills_instructions>",
            "<plugins_instructions>",
            "<INSTRUCTIONS>",
        ];
        if hidden_prefixes
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
        {
            return String::new();
        }
        if trimmed.starts_with("<goal_context>")
            && let Ok(regex) = Regex::new(r"(?s)<objective>\s*(.*?)\s*</objective>")
        {
            return regex
                .captures(trimmed)
                .and_then(|caps| caps.get(1))
                .map(|value| value.as_str().trim().to_owned())
                .unwrap_or_default();
        }
        content.to_owned()
    }

    fn parse_arguments(value: Option<&Value>) -> Map<String, Value> {
        match value {
            Some(Value::Object(map)) => map.clone(),
            Some(Value::String(text)) if !text.trim().is_empty() => {
                serde_json::from_str::<Map<String, Value>>(text).unwrap_or_else(|_| {
                    Map::from_iter([("arguments".into(), Value::String(text.clone()))])
                })
            }
            Some(value) if !value.is_null() => Map::from_iter([("value".into(), value.clone())]),
            _ => Map::new(),
        }
    }

    fn tool_name(payload: &Map<String, Value>) -> String {
        let name = payload
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("tool");
        let namespace = payload
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or_default();
        namespace
            .strip_prefix("mcp__")
            .map(|server| format!("mcp:{}.{}", server.trim_end_matches("__"), name))
            .unwrap_or_else(|| name.to_owned())
    }

    fn timestamp(value: Option<&Value>) -> String {
        value
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| Utc::now().to_rfc3339())
    }

    fn timestamp_ms(value: Option<&str>) -> Option<i64> {
        DateTime::parse_from_rfc3339(value?)
            .ok()
            .map(|time| time.timestamp_millis())
    }

    fn file_times(path: &Path) -> (i64, i64) {
        let Ok(metadata) = fs::metadata(path) else {
            return (0, 0);
        };
        let to_ms = |time: std::io::Result<std::time::SystemTime>| {
            time.ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_millis() as i64)
                .unwrap_or(0)
        };
        let modified = to_ms(metadata.modified());
        (to_ms(metadata.created()).max(modified), modified)
    }

    fn truncate(text: &str, max: usize) -> String {
        let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut chars = normalized.chars();
        let value: String = chars.by_ref().take(max).collect();
        if chars.next().is_some() {
            format!("{value}...")
        } else {
            value
        }
    }

    fn image_data_url(&self, path: &Path, cwd: Option<&Path>) -> Option<String> {
        const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        let mime = match extension.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "svg" => "image/svg+xml",
            "bmp" => "image/bmp",
            "ico" => "image/x-icon",
            "avif" => "image/avif",
            _ => return None,
        };
        let canonical = path.canonicalize().ok()?;
        let in_root = [&self.root as &Path, cwd.unwrap_or(&self.root)]
            .into_iter()
            .any(|root| {
                root.canonicalize()
                    .is_ok_and(|root| canonical == root || canonical.starts_with(root))
            });
        if !in_root || fs::metadata(&canonical).ok()?.len() > MAX_IMAGE_BYTES {
            return None;
        }
        Some(format!(
            "data:{mime};base64,{}",
            STANDARD.encode(fs::read(canonical).ok()?)
        ))
    }

    fn embed_images(&self, content: &str, cwd: Option<&Path>) -> String {
        let Ok(regex) = Regex::new(r"(!\[[^\]]*\]\()([^\)\s]+)(\))") else {
            return content.to_owned();
        };
        regex
            .replace_all(content, |caps: &regex::Captures<'_>| {
                let path = Path::new(&caps[2]);
                if !path.is_absolute() {
                    return caps[0].to_owned();
                }
                self.image_data_url(path, cwd)
                    .map(|url| format!("{}{}{}", &caps[1], url, &caps[3]))
                    .unwrap_or_else(|| caps[0].to_owned())
            })
            .into_owned()
    }

    fn parse_messages(&self, records: &[Value], cwd: Option<&Path>) -> Vec<SessionMessage> {
        let mut messages = Vec::new();
        let mut pending_tools: HashMap<String, String> = HashMap::new();
        let mut current_model = None;
        for record in records {
            let record_type = record.get("type").and_then(Value::as_str);
            if record_type == Some("turn_context") {
                current_model = record
                    .pointer("/payload/model")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                continue;
            }
            if record_type != Some("response_item") {
                continue;
            }
            let Some(payload) = record.get("payload").and_then(Value::as_object) else {
                continue;
            };
            let timestamp = Self::timestamp(record.get("timestamp"));
            match payload.get("type").and_then(Value::as_str) {
                Some("message") => {
                    let Some(role @ ("user" | "assistant")) =
                        payload.get("role").and_then(Value::as_str)
                    else {
                        continue;
                    };
                    let raw = Self::value_text(payload.get("content"));
                    let content = if role == "user" {
                        Self::normalize_user(&raw)
                    } else {
                        self.embed_images(&raw, cwd)
                    };
                    if content.is_empty() {
                        continue;
                    }
                    let mut message = SessionMessage::text(
                        if role == "user" {
                            MessageType::User
                        } else {
                            MessageType::Assistant
                        },
                        timestamp,
                        content,
                    );
                    message.model = current_model.clone();
                    messages.push(message);
                }
                Some("reasoning") => {
                    let reasoning = Self::reasoning_text(payload);
                    if reasoning.is_empty() {
                        continue;
                    }
                    let mut message = SessionMessage::text(MessageType::Assistant, timestamp, "");
                    message.content = None;
                    message.reasoning_content = Some(reasoning);
                    message.model = current_model.clone();
                    messages.push(message);
                }
                Some(kind @ ("function_call" | "custom_tool_call")) => {
                    let name = Self::tool_name(payload);
                    let call_id = payload
                        .get("call_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let arguments = if kind == "custom_tool_call" {
                        payload.get("input")
                    } else {
                        payload.get("arguments")
                    };
                    let mut input = Self::parse_arguments(arguments);
                    if kind == "custom_tool_call"
                        && name == "apply_patch"
                        && let Some(Value::String(patch)) = arguments
                    {
                        input = Map::from_iter([("patch".into(), Value::String(patch.clone()))]);
                    }
                    let mut message = SessionMessage::text(MessageType::ToolUse, timestamp, "");
                    message.content = None;
                    message.tool_name = Some(name.clone());
                    message.tool_input = Some(input);
                    message.call_id = call_id.clone();
                    message.model = current_model.clone();
                    if let Some(status) = payload.get("status").and_then(Value::as_str) {
                        message.metadata.insert("subtype".into(), json!(status));
                    }
                    if let Some(id) = call_id {
                        pending_tools.insert(id, name);
                    }
                    messages.push(message);
                }
                Some(kind @ ("function_call_output" | "custom_tool_call_output")) => {
                    let call_id = payload
                        .get("call_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let mut output = match payload.get("output") {
                        Some(Value::String(text)) => text.clone(),
                        Some(value) => serde_json::to_string_pretty(value).unwrap_or_default(),
                        None => String::new(),
                    };
                    if kind == "custom_tool_call_output"
                        && let Ok(value) = serde_json::from_str::<Value>(&output)
                        && let Some(text) = value.get("output").and_then(Value::as_str)
                    {
                        output = text.to_owned()
                    }
                    let mut message = SessionMessage::text(MessageType::ToolResult, timestamp, "");
                    message.content = None;
                    message.tool_name = call_id
                        .as_ref()
                        .and_then(|id| pending_tools.get(id))
                        .cloned()
                        .or_else(|| Some(Self::tool_name(payload)));
                    message.tool_output = Some(ToolOutput {
                        output: Some(output),
                        preview: None,
                        truncated: false,
                        extra: Map::new(),
                    });
                    message.call_id = call_id;
                    message.model = current_model.clone();
                    if let Some(status) = payload.get("status").and_then(Value::as_str) {
                        message.metadata.insert("subtype".into(), json!(status));
                    }
                    messages.push(message);
                }
                _ => {}
            }
        }
        messages
    }

    fn parse_summary(records: &[Value]) -> (usize, String, String) {
        let mut count = 0;
        let mut first = String::new();
        let mut last = String::new();
        for record in records {
            if record.get("type").and_then(Value::as_str) != Some("response_item") {
                continue;
            }
            let Some(payload) = record.get("payload").and_then(Value::as_object) else {
                continue;
            };
            let preview = match payload.get("type").and_then(Value::as_str) {
                Some("message") => {
                    let role = payload.get("role").and_then(Value::as_str);
                    if !matches!(role, Some("user" | "assistant")) {
                        String::new()
                    } else {
                        let text = Self::value_text(payload.get("content"));
                        if role == Some("user") {
                            Self::normalize_user(&text)
                        } else {
                            text
                        }
                    }
                }
                Some("reasoning") => Self::reasoning_text(payload),
                Some("function_call" | "custom_tool_call") => {
                    format!("[Tool: {}]", Self::tool_name(payload))
                }
                Some("function_call_output" | "custom_tool_call_output") => "[Tool result]".into(),
                _ => String::new(),
            };
            if preview.is_empty() {
                continue;
            }
            count += 1;
            let preview = Self::truncate(&preview, 200);
            if first.is_empty() {
                first = preview.clone();
            }
            last = preview;
        }
        (count, first, last)
    }

    fn line_is_summary_item(line: &str) -> bool {
        // Tool results can be hundreds of megabytes on a single JSONL line. The
        // payload discriminator is always near the beginning, so never scan the
        // complete line just to classify it.
        let compact = &line.as_bytes()[..line.len().min(512)];
        let has = |needle: &[u8]| compact.windows(needle.len()).any(|window| window == needle);
        has(b"\"type\":\"message\"")
            || has(b"\"type\":\"reasoning\"")
            || has(b"\"type\":\"function_call\"")
            || has(b"\"type\":\"custom_tool_call\"")
            || has(b"\"type\":\"function_call_output\"")
            || has(b"\"type\":\"custom_tool_call_output\"")
    }

    fn line_preview(line: &str) -> String {
        serde_json::from_str::<Value>(line)
            .ok()
            .map(|record| Self::parse_summary(std::slice::from_ref(&record)).1)
            .unwrap_or_default()
    }

    fn make_session_summary(
        &self,
        path: &Path,
        index: &HashMap<String, IndexEntry>,
    ) -> Option<Session> {
        // Session histories can grow to hundreds of megabytes. Listing sessions
        // only needs metadata and a preview; full parsing is deferred until the
        // user opens a session.
        let source = Self::read_summary_prefix(path)?;
        if source.is_empty() || source.contains("codex-auto-review") {
            return None;
        }
        let first_record = source
            .lines()
            .take(8)
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|record| record.get("type").and_then(Value::as_str) == Some("session_meta"));
        let meta = first_record
            .as_ref()
            .and_then(|record| record.get("payload"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let id = meta
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| Self::file_id(path))?;
        let index_entry = index.get(&id);
        let candidates = source
            .lines()
            .filter(|line| Self::line_is_summary_item(line))
            .collect::<Vec<_>>();
        let first = candidates
            .iter()
            .find_map(|line| {
                let preview = Self::line_preview(line);
                (!preview.is_empty()).then_some(preview)
            })
            .unwrap_or_default();
        let last = candidates
            .iter()
            .rev()
            .find_map(|line| {
                let preview = Self::line_preview(line);
                (!preview.is_empty()).then_some(preview)
            })
            .unwrap_or_default();
        let (created_file, updated_file) = Self::file_times(path);
        let created_at = Self::timestamp_ms(meta.get("timestamp").and_then(Value::as_str))
            .unwrap_or(created_file);
        let updated_at =
            Self::timestamp_ms(index_entry.and_then(|entry| entry.updated_at.as_deref()))
                .unwrap_or(updated_file);
        let fallback_file_name = path.file_name()?.to_string_lossy().into_owned();
        Some(Session {
            id,
            app_type: AppType::Codex,
            file_name: index_entry
                .and_then(|entry| entry.thread_name.clone())
                .unwrap_or(fallback_file_name),
            file_path: path.to_path_buf(),
            created_at,
            updated_at,
            message_count: candidates.len(),
            first_message: index_entry
                .and_then(|entry| entry.thread_name.clone())
                .unwrap_or_else(|| Self::truncate(&first, 200)),
            last_message: Self::truncate(&last, 200),
            directory: meta.get("cwd").and_then(Value::as_str).map(PathBuf::from),
            uuid: None,
            kind: SessionKind::Main,
            parent_session_id: None,
            agent_type: None,
        })
    }

    fn make_session(
        &self,
        path: &Path,
        records: &[Value],
        index: &HashMap<String, IndexEntry>,
        include_messages: bool,
    ) -> Option<(Session, Vec<SessionMessage>)> {
        if records.is_empty() || Self::is_review(records) {
            return None;
        }
        let meta = Self::metadata(records);
        let id = meta
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| Self::file_id(path))?;
        let cwd = meta.get("cwd").and_then(Value::as_str).map(PathBuf::from);
        let messages = include_messages.then(|| self.parse_messages(records, cwd.as_deref()));
        let (message_count, first, last) = if let Some(messages) = &messages {
            let first = messages
                .iter()
                .find(|message| message.message_type == MessageType::User)
                .and_then(|message| message.content.as_deref())
                .unwrap_or_default();
            let last = messages
                .iter()
                .rev()
                .find_map(|message| {
                    message
                        .content
                        .as_deref()
                        .or(message.reasoning_content.as_deref())
                        .or_else(|| message.tool_output.as_ref()?.output.as_deref())
                })
                .unwrap_or_default();
            (
                messages.len(),
                Self::truncate(first, 200),
                Self::truncate(last, 200),
            )
        } else {
            Self::parse_summary(records)
        };
        let index_entry = index.get(&id);
        let (created_file, updated_file) = Self::file_times(path);
        let created_at = Self::timestamp_ms(meta.get("timestamp").and_then(Value::as_str))
            .unwrap_or(created_file);
        let updated_at =
            Self::timestamp_ms(index_entry.and_then(|entry| entry.updated_at.as_deref()))
                .or_else(|| {
                    records.last().and_then(|record| {
                        Self::timestamp_ms(record.get("timestamp").and_then(Value::as_str))
                    })
                })
                .unwrap_or(updated_file);
        let title = index_entry
            .and_then(|entry| entry.thread_name.clone())
            .unwrap_or_else(|| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        let session = Session {
            id,
            app_type: AppType::Codex,
            file_name: title,
            file_path: path.to_path_buf(),
            created_at,
            updated_at,
            message_count,
            first_message: index_entry
                .and_then(|entry| entry.thread_name.clone())
                .unwrap_or(first),
            last_message: last,
            directory: cwd,
            uuid: None,
            kind: SessionKind::Main,
            parent_session_id: None,
            agent_type: None,
        };
        Some((session, messages.unwrap_or_default()))
    }
}

impl SessionProvider for CodexProvider {
    fn app_type(&self) -> AppType {
        AppType::Codex
    }
    fn is_available(&self) -> bool {
        self.sessions_path().exists()
    }

    fn sessions(&self) -> Result<Vec<Session>> {
        let index = self.load_index();
        let mut sessions = self
            .session_files()
            .into_par_iter()
            .filter_map(|path| self.make_session_summary(&path, &index))
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        Ok(sessions)
    }

    fn session_detail(&self, session_id: &str) -> Result<Option<SessionDetail>> {
        let index = self.load_index();
        let files = self.session_files();
        if let Some(path) = files
            .iter()
            .find(|path| Self::file_id(path).as_deref() == Some(session_id))
        {
            let records = Self::read_jsonl(path)?;
            return Ok(self
                .make_session(path, &records, &index, true)
                .map(|(session, messages)| SessionDetail { session, messages }));
        }
        for path in files {
            let first = fs::read_to_string(&path)
                .ok()
                .and_then(|source| source.lines().next().map(str::to_owned))
                .and_then(|line| serde_json::from_str::<Value>(&line).ok());
            let meta_id = first
                .as_ref()
                .and_then(|record| record.get("payload"))
                .and_then(|payload| payload.get("id"))
                .and_then(Value::as_str);
            if meta_id != Some(session_id) {
                continue;
            }
            let records = Self::read_jsonl(&path)?;
            return Ok(self
                .make_session(&path, &records, &index, true)
                .map(|(session, messages)| SessionDetail { session, messages }));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_codex_runtime_context_messages() {
        assert!(
            CodexProvider::normalize_user("<environment_context>secret</environment_context>")
                .is_empty()
        );
        assert_eq!(CodexProvider::normalize_user("hello"), "hello");
    }

    #[test]
    fn parses_json_and_raw_tool_arguments() {
        assert_eq!(
            CodexProvider::parse_arguments(Some(&json!("{\"path\":\"a\"}"))).get("path"),
            Some(&json!("a"))
        );
        assert_eq!(
            CodexProvider::parse_arguments(Some(&json!("raw"))).get("arguments"),
            Some(&json!("raw"))
        );
    }
}
