use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::Result;
use chrono::{DateTime, TimeZone as _, Utc};
use rayon::prelude::*;
use regex::Regex;
use serde_json::{Map, Value, json};
use walkdir::WalkDir;

use super::SessionProvider;
use crate::{
    AppType, MessageType, Session, SessionDetail, SessionMessage,
    model::{SessionKind, TokenUsage, ToolOutput},
};

const SUMMARY_PREFIX_BYTES: u64 = 256 * 1024;

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

    fn token_usage(record: &Value) -> Option<TokenUsage> {
        [
            "/providerData/usage",
            "/message/usage",
            "/providerData/rawUsage",
        ]
        .into_iter()
        .filter_map(|path| record.pointer(path))
        .find_map(|usage| {
            let number = |paths: &[&str]| {
                paths
                    .iter()
                    .find_map(|path| usage.pointer(path).and_then(Value::as_u64))
            };
            let input_tokens = number(&["/inputTokens", "/input_tokens", "/prompt_tokens"]);
            let output_tokens = number(&["/outputTokens", "/output_tokens", "/completion_tokens"]);
            let total_tokens = number(&["/totalTokens", "/total_tokens"])
                .or_else(|| input_tokens?.checked_add(output_tokens?));
            let cache_read_tokens = number(&[
                "/inputTokensDetails/0/cached_tokens",
                "/inputTokensDetails/cached_tokens",
                "/prompt_tokens_details/cached_tokens",
                "/prompt_cache_hit_tokens",
                "/cache_read_input_tokens",
            ]);
            let parsed = TokenUsage {
                input_tokens,
                output_tokens,
                total_tokens,
                cache_read_tokens,
            };
            (parsed != TokenUsage::default()).then_some(parsed)
        })
    }

    fn normalize(&self, records: &[Value], fallback: i64) -> Vec<SessionMessage> {
        let mut messages = Vec::new();
        let mut pending_agents: HashMap<String, Map<String, Value>> = HashMap::new();
        let mut current_model = None;
        let mut pending_reasoning = String::new();
        let mut pending_usage = Vec::new();
        let mut seen_responses = HashSet::new();

        for record in records {
            current_model = Self::string(record.pointer("/providerData/model")).or(current_model);
            let timestamp = Self::iso_time(record.get("timestamp"), fallback);
            let kind = record
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let model_record = matches!(kind, "reasoning" | "assistant" | "function_call")
                || (kind == "message"
                    && record.get("role").and_then(Value::as_str) == Some("assistant"));
            let usage = model_record
                .then(|| Self::token_usage(record))
                .flatten()
                .filter(|_| {
                    let response_id = Self::string(record.pointer("/providerData/messageId"))
                        .or_else(|| Self::string(record.pointer("/message/id")));
                    response_id.is_none_or(|id| seen_responses.insert(id))
                });
            if let Some(usage) = usage {
                pending_usage.push(usage);
            }
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
                if !pending_reasoning.is_empty() || !pending_usage.is_empty() {
                    let mut pending =
                        SessionMessage::text(MessageType::Assistant, timestamp.clone(), "");
                    pending.content = None;
                    pending.reasoning_content = Some(std::mem::take(&mut pending_reasoning));
                    pending.model = current_model.clone();
                    pending.usage = TokenUsage::aggregate(&pending_usage);
                    pending_usage.clear();
                    messages.push(pending);
                }
                let role = record.get("role").and_then(Value::as_str).unwrap_or("user");
                let mut text = Self::message_text(record);
                if role == "user" {
                    text = Self::clean_user_text(&text);
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
                crate::attachments::normalize(
                    &mut message,
                    record
                        .get("content")
                        .or_else(|| record.pointer("/message/content")),
                );
                if message.content.as_deref().unwrap_or_default().is_empty()
                    && message.attachments.is_empty()
                {
                    continue;
                }
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
                {
                    let mut message = SessionMessage::text(MessageType::Assistant, timestamp, text);
                    crate::attachments::normalize(
                        &mut message,
                        record
                            .get("content")
                            .or_else(|| record.pointer("/message/content")),
                    );
                    if message.content.as_deref().unwrap_or_default().is_empty()
                        && pending_reasoning.is_empty()
                        && pending_usage.is_empty()
                        && message.attachments.is_empty()
                    {
                        continue;
                    }
                    message.reasoning_content = (!pending_reasoning.is_empty())
                        .then(|| std::mem::take(&mut pending_reasoning));
                    if let Some(status) = Self::string(record.get("status")) {
                        message.metadata.insert("subtype".into(), json!(status));
                    }
                    message.model = current_model.clone();
                    message.usage = TokenUsage::aggregate(&pending_usage);
                    pending_usage.clear();
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
                message.usage = TokenUsage::aggregate(&pending_usage);
                pending_usage.clear();
                if name.to_ascii_lowercase().contains("agent") {
                    pending_agents.insert(call_id, input);
                }
                messages.push(message);
                continue;
            }

            if kind == "function_call_result" {
                if !pending_reasoning.is_empty() || !pending_usage.is_empty() {
                    let mut pending =
                        SessionMessage::text(MessageType::Assistant, timestamp.clone(), "");
                    pending.content = None;
                    pending.reasoning_content = Some(std::mem::take(&mut pending_reasoning));
                    pending.model = current_model.clone();
                    pending.usage = TokenUsage::aggregate(&pending_usage);
                    pending_usage.clear();
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
        if !pending_reasoning.is_empty() || !pending_usage.is_empty() {
            let mut pending =
                SessionMessage::text(MessageType::Assistant, Self::iso_time(None, fallback), "");
            pending.content = None;
            pending.reasoning_content = Some(pending_reasoning);
            pending.model = current_model;
            pending.usage = TokenUsage::aggregate(&pending_usage);
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

    fn active_titles(&self) -> HashMap<String, String> {
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
            // Active-session timestamps track the CLI process heartbeat, not new messages.
            // Read this metadata for the topic only; transcript updates drive recency.
            active.insert(id, title);
        }
        active
    }

    fn previews(messages: &[SessionMessage]) -> (String, String) {
        let clean = |message: &SessionMessage| {
            if message
                .content
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
            {
                return message
                    .attachments
                    .iter()
                    .map(|item| item.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
            }
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

    // Only the latest unanswered request may use the live file. Completed requests
    // must keep their recorded output so later edits cannot rewrite history.
    fn attach_pending_plan(&self, messages: &mut [SessionMessage]) {
        let Some(index) = messages
            .iter()
            .rposition(|message| message.message_type == MessageType::ToolUse)
        else {
            return;
        };
        let request = &messages[index];
        if request.tool_name.as_deref() != Some("ExitPlanMode")
            || messages[index + 1..].iter().any(|message| {
                message.message_type == MessageType::ToolResult
                    && message.call_id == request.call_id
            })
        {
            return;
        }
        messages[index]
            .metadata
            .insert("codebuddyPendingPlan".into(), json!(true));
        let Some(entered) = messages[..index].iter().rev().find(|message| {
            message.message_type == MessageType::ToolResult
                && message.tool_name.as_deref() == Some("EnterPlanMode")
        }) else {
            return;
        };
        let output = entered
            .tool_output
            .as_ref()
            .and_then(|output| output.output.as_deref())
            .unwrap_or_default();
        let Ok(pattern) = Regex::new(r"(?m)(/[^\n`]*?/\.codebuddy/plans/[^\n`]*?\.md)\b") else {
            return;
        };
        let Some(path) = pattern
            .find(output)
            .map(|matched| Path::new(matched.as_str()))
        else {
            return;
        };
        let Ok(path) = super::search_path(&self.root.join("plans"), path) else {
            return;
        };
        // A corrupt transcript must not cause an unbounded read of an external file.
        let mut content = String::new();
        if fs::File::open(path)
            .and_then(|file| file.take(2 * 1024 * 1024 + 1).read_to_string(&mut content))
            .is_ok()
            && content.len() <= 2 * 1024 * 1024
            && !content.trim().is_empty()
        {
            messages[index].content = Some(content);
        }
    }

    fn detail_for(&self, file: &SessionFile) -> Option<SessionDetail> {
        if file.size == 0 {
            return None;
        }
        let records = Self::values(&file.path);
        let mut messages = self.normalize(&records, file.updated_at);
        self.attach_pending_plan(&mut messages);
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
            subtree_usage: None,
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
        // Only discard an empty normalized history when the bounded summary covers
        // the whole file. Large transcripts may start with a record beyond the cap.
        if messages.is_empty() && file.size <= SUMMARY_PREFIX_BYTES {
            return None;
        }

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
            if let Some(title) = active_value {
                if !title.is_empty() {
                    session.file_name = title.clone();
                    session.last_message = title.clone();
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

    fn session_detail_for_search(&self, session: &Session) -> Result<Option<SessionDetail>> {
        let path = super::search_path(&self.projects_path(), &session.file_path)?;
        anyhow::ensure!(
            path.file_stem().and_then(|stem| stem.to_str()) == Some(session.id.as_str()),
            "Session path does not match its ID"
        );
        let (created_at, updated_at, size) = Self::file_times(&path);
        Ok(self.detail_for(&SessionFile {
            path,
            project_cwd: session.directory.clone().unwrap_or_default(),
            id: session.id.clone(),
            internal_id: session.uuid.clone(),
            parent_id: session.parent_session_id.clone(),
            agent_type: session.agent_type.clone(),
            kind: session.kind,
            size,
            created_at,
            updated_at,
        }))
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
    fn usage_prefers_normalized_mirror_and_derives_total() {
        let usage = CodeBuddyProvider::token_usage(&json!({
            "providerData": {"usage": {"inputTokens": 100, "outputTokens": 20, "inputTokensDetails": [{"cached_tokens": 80}]},
                "rawUsage": {"prompt_tokens": 900, "completion_tokens": 90, "cache_read_input_tokens": 0}},
            "message": {"usage": {"input_tokens": 900, "output_tokens": 90}}
        })).unwrap();
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.total_tokens, Some(120));
        assert_eq!(usage.cache_read_tokens, Some(80));
        let fallback = CodeBuddyProvider::token_usage(&json!({"providerData": {"rawUsage": {
            "prompt_tokens": 100, "completion_tokens": 20, "prompt_cache_hit_tokens": 80, "cache_read_input_tokens": 0
        }}})).unwrap();
        assert_eq!(usage, fallback);
        assert!(
            CodeBuddyProvider::token_usage(&json!({"message": {"usage": {"input_tokens": -1}}}))
                .is_none()
        );
    }

    #[test]
    fn response_ids_deduplicate_usage_but_equal_counts_do_not() {
        let provider = CodeBuddyProvider::default();
        let record = |id: &str| {
            json!({"type": "function_call", "name": "Read", "arguments": {},
            "providerData": {"messageId": id, "usage": {"inputTokens": 100, "outputTokens": 20}}})
        };
        let messages = provider.normalize(&[record("a"), record("a"), record("b")], 0);
        assert_eq!(messages.len(), 3);
        assert!(messages[0].usage.is_some());
        assert!(messages[1].usage.is_none());
        assert_eq!(
            super::TokenUsage::aggregate(
                messages.iter().filter_map(|message| message.usage.as_ref())
            )
            .unwrap()
            .input_tokens,
            Some(200)
        );
    }

    #[test]
    fn reasoning_usage_survives_flush_and_empty_assistant() {
        let provider = CodeBuddyProvider::default();
        let reasoning = json!({"type": "reasoning", "content": "Thinking", "providerData": {
            "messageId": "response", "usage": {"inputTokens": 100, "outputTokens": 20}
        }});
        let assistant = json!({"type": "message", "role": "assistant", "content": "", "providerData": {
            "messageId": "response", "usage": {"inputTokens": 100, "outputTokens": 20}
        }});
        for records in [
            vec![reasoning.clone()],
            vec![reasoning.clone(), assistant],
            vec![
                reasoning,
                json!({"type": "message", "role": "user", "content": "Next"}),
            ],
        ] {
            let messages = provider.normalize(&records, 0);
            assert_eq!(
                messages
                    .iter()
                    .filter(|message| message.usage.is_some())
                    .count(),
                1
            );
            assert_eq!(messages[0].usage.as_ref().unwrap().total_tokens, Some(120));
        }
        let messages = provider.normalize(&[json!({"type": "assistant", "message": {"usage": {"input_tokens": 3, "output_tokens": 0}}})], 0);
        assert_eq!(messages[0].usage.as_ref().unwrap().total_tokens, Some(3));
    }

    #[test]
    fn pending_plan_reads_live_file_without_rewriting_completed_requests() {
        let base = std::env::temp_dir().join(format!("yes-pending-plan-{}", std::process::id()));
        let root = base.join(".codebuddy");
        fs::create_dir_all(root.join("plans")).unwrap();
        let path = root.join("plans/test plan.md");
        let provider = CodeBuddyProvider::with_root(root.clone());
        let mut records = vec![
            json!({"type":"function_call_result", "name":"EnterPlanMode", "callId":"enter",
                "output":{"type":"text","text":format!("Create your plan at {} using the Write tool.", path.display())}}),
            json!({"type":"function_call", "name":"ExitPlanMode", "callId":"exit", "arguments":{}}),
        ];
        let load = |records: &[Value]| {
            let mut messages = provider.normalize(records, 0);
            provider.attach_pending_plan(&mut messages);
            messages
        };
        // No file yet: retain the real request and retry on detail refresh.
        let missing = load(&records);
        assert!(missing[1].content.is_none());
        assert_eq!(
            missing[1].metadata.get("codebuddyPendingPlan"),
            Some(&json!(true))
        );
        fs::write(&path, "# First plan\n\n- Check parsing").unwrap();
        assert!(provider.normalize(&records, 0)[1].content.is_none());
        assert_eq!(
            load(&records)[1].content.as_deref(),
            Some("# First plan\n\n- Check parsing")
        );
        fs::write(&path, "# Revised plan").unwrap();
        assert_eq!(load(&records)[1].content.as_deref(), Some("# Revised plan"));

        records.push(
            json!({"type":"function_call_result", "name":"ExitPlanMode", "callId":"exit",
            "output":{"type":"text", "text":"User has approved your plan.\n\n# Recorded plan"}}),
        );
        let completed = load(&records);
        assert!(completed[1].content.is_none());
        assert!(!completed[1].metadata.contains_key("codebuddyPendingPlan"));
        assert_eq!(
            completed[2].tool_output.as_ref().unwrap().output.as_deref(),
            Some("User has approved your plan.\n\n# Recorded plan")
        );
        records.push(json!({"type":"function_call", "name":"ExitPlanMode", "callId":"exit-2", "arguments":{}}));
        let retried = load(&records);
        assert!(retried[1].content.is_none());
        assert_eq!(retried[3].content.as_deref(), Some("# Revised plan"));

        // Canonical boundaries reject both traversal and symlink escapes.
        let outside = base.join("outside.md");
        fs::write(&outside, "external content").unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(load(&records)[3].content.is_none());
        records[0]["output"]["text"] = json!(format!(
            "Create at {}/plans/../../outside.md",
            root.display()
        ));
        assert!(load(&records)[3].content.is_none());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn control_only_sessions_are_hidden_until_real_messages_arrive() {
        let root =
            std::env::temp_dir().join(format!("yes-codebuddy-control-only-{}", std::process::id()));
        let project = root.join("projects/Users-test-project");
        fs::create_dir_all(&project).unwrap();
        let path = project.join("control-only.jsonl");
        let records = [
            "<system-reminder>CLI instructions</system-reminder>",
            "<command-name>/help</command-name>",
            "<local-command-stdout>CLI output</local-command-stdout>",
        ]
        .map(|text| {
            json!({"type":"message", "role":"user", "sessionId":"control-only",
                "content":[{"type":"input_text","text":text}]})
            .to_string()
        })
        .join("\n");
        fs::write(&path, &records).unwrap();
        let provider = CodeBuddyProvider::with_root(root.clone());
        assert!(provider.sessions().unwrap().is_empty());
        assert!(provider.session_detail("control-only").unwrap().is_none());
        let message = json!({"type":"message", "role":"user", "content":"A real question"});
        fs::write(&path, format!("{records}\n{message}\n")).unwrap();
        let sessions = provider.sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].first_message, "A real question");
        assert_eq!(
            provider
                .session_detail("control-only")
                .unwrap()
                .unwrap()
                .messages
                .len(),
            1
        );
        // An oversized first record is absent from the bounded summary but must
        // not make a real transcript disappear from the list.
        fs::write(&path, json!({"type":"message", "role":"user", "content":"x".repeat(SUMMARY_PREFIX_BYTES as usize + 1)}).to_string()).unwrap();
        assert_eq!(provider.sessions().unwrap().len(), 1);
        assert!(provider.session_detail("control-only").unwrap().is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn idle_session_heartbeat_does_not_override_transcript_recency() {
        use std::time::Duration;
        let root =
            std::env::temp_dir().join(format!("yes-codebuddy-recency-{}", std::process::id()));
        let project = root.join("projects/Users-test-project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(root.join("sessions")).unwrap();
        for (id, seconds) in [("idle", 100), ("recent", 200)] {
            let path = project.join(format!("{id}.jsonl"));
            fs::write(
                &path,
                json!({"type":"message", "role":"user", "sessionId":id,
                "timestamp":seconds * 1000, "content":"hello"})
                .to_string(),
            )
            .unwrap();
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(
                    fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)),
                )
                .unwrap();
        }
        let active = root.join("sessions/123.json");
        let provider = CodeBuddyProvider::with_root(root.clone());
        for heartbeat in [300_000, 400_000] {
            fs::write(
                &active,
                json!({"sessionId":"idle", "updatedAt":heartbeat,
                "lastHeartbeat":heartbeat, "meta":{"currentTopic":"Idle topic"}})
                .to_string(),
            )
            .unwrap();
            let sessions = provider.sessions().unwrap();
            assert_eq!(
                sessions
                    .iter()
                    .map(|session| session.id.as_str())
                    .collect::<Vec<_>>(),
                ["recent", "idle"]
            );
            assert_eq!(sessions[1].updated_at, 100_000);
            assert_eq!(sessions[1].file_name, "Idle topic");
        }
        // Real transcript activity still moves the session above the previous latest session.
        let path = project.join("idle.jsonl");
        fs::write(&path, format!("{}\n{}\n", fs::read_to_string(&path).unwrap(),
            json!({"type":"message", "role":"assistant", "timestamp":500_000, "content":"reply"}))).unwrap();
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(500)))
            .unwrap();
        assert_eq!(provider.sessions().unwrap()[0].id, "idle");
        fs::remove_dir_all(root).unwrap();
    }

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
    #[test]
    fn blob_references_are_preserved_without_loading_files() {
        let provider = CodeBuddyProvider::with_root(std::path::PathBuf::from("/tmp/absent"));
        let messages = provider.normalize(&[json!({"type":"message","role":"user","content":[{"type":"image_blob_ref","blob_path":"/tmp/missing-image.png","mime":"image/png"},{"type":"file","path":"/tmp/missing.pdf"}]})], 0);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].attachments.len(), 2);
    }
}
