use std::{
    fs,
    io::Read as _,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::Result;
use chrono::Utc;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::SessionProvider;
use crate::{
    AppType, MessageType, Session, SessionDetail, SessionMessage,
    model::{SessionKind, ToolOutput},
};

#[derive(Debug, Clone)]
pub struct ClaudeProvider {
    root: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
struct HistoryEntry {
    display: Option<String>,
    timestamp: i64,
    project: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: String,
}

impl Default for ClaudeProvider {
    fn default() -> Self {
        Self {
            root: dirs::home_dir().unwrap_or_default().join(".claude"),
        }
    }
}

impl ClaudeProvider {
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }
    fn projects_path(&self) -> PathBuf {
        self.root.join("projects")
    }
    fn transcripts_path(&self) -> PathBuf {
        self.root.join("transcripts")
    }

    fn history(&self) -> Vec<HistoryEntry> {
        fs::read_to_string(self.root.join("history.jsonl"))
            .ok()
            .map(|source| {
                source
                    .lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn read_values(path: &Path) -> Vec<Value> {
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

    fn text(value: Option<&Value>, accepted_types: &[&str]) -> String {
        match value {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| {
                    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
                    (accepted_types.is_empty() || accepted_types.contains(&item_type))
                        .then(|| {
                            item.get("text")
                                .or_else(|| item.get("thinking"))
                                .and_then(Value::as_str)
                        })
                        .flatten()
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        }
    }

    fn model(value: Option<&Value>) -> Option<String> {
        match value {
            Some(Value::String(text)) => Some(text.clone()),
            Some(Value::Object(object)) => object
                .get("modelID")
                .or_else(|| object.get("model"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            _ => None,
        }
    }

    fn message_model(record: &Value, inherited: Option<String>) -> Option<String> {
        Self::model(record.pointer("/message/model"))
            .or_else(|| Self::model(record.get("model")))
            .or_else(|| Self::model(record.get("metadata")))
            .or(inherited)
    }

    fn timestamp(record: &Value) -> String {
        record
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| Utc::now().to_rfc3339())
    }

    fn capture(content: &str, tag: &str) -> Option<String> {
        Regex::new(&format!(r"(?s)<{tag}>(.*?)</{tag}>"))
            .ok()?
            .captures(content)?
            .get(1)
            .map(|value| value.as_str().to_owned())
    }

    fn parse_new_message(record: &Value, model: Option<String>) -> Option<SessionMessage> {
        let kind = record.get("type")?.as_str()?;
        let timestamp = Self::timestamp(record);
        if kind == "user" {
            if record.get("promptId").is_none() {
                let output = record.get("toolUseResult").and_then(Value::as_str)?;
                let mut message = SessionMessage::text(
                    MessageType::ToolResult,
                    timestamp,
                    output.chars().take(300).collect::<String>(),
                );
                message.tool_name = Some("unknown".into());
                message.tool_output = Some(ToolOutput {
                    output: Some(output.into()),
                    preview: None,
                    truncated: false,
                    extra: Map::new(),
                });
                message.call_id = record
                    .get("sourceToolAssistantUUID")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                message.model = model;
                return Some(message);
            }
            let content = Self::text(record.pointer("/message/content"), &["text"]);
            let (message_type, content, metadata) = if content.contains("<local-command-caveat>") {
                (
                    MessageType::System,
                    Self::capture(&content, "local-command-caveat")
                        .unwrap_or_else(|| "Local command caveat".into()),
                    Map::from_iter([("subtype".into(), json!("caveat"))]),
                )
            } else if content.contains("<local-command-stdout>") {
                (
                    MessageType::System,
                    Self::capture(&content, "local-command-stdout").unwrap_or(content),
                    Map::from_iter([("subtype".into(), json!("command_output"))]),
                )
            } else if content.contains("<command-name>") {
                let name =
                    Self::capture(&content, "command-name").unwrap_or_else(|| "unknown".into());
                let detail = Self::capture(&content, "command-message").unwrap_or_default();
                let args = Self::capture(&content, "command-args").unwrap_or_default();
                (
                    MessageType::System,
                    format!("{name} {detail} {args}").trim().to_owned(),
                    Map::from_iter([
                        ("subtype".into(), json!("command")),
                        ("command".into(), json!(name)),
                    ]),
                )
            } else {
                (MessageType::User, content, Map::new())
            };
            let mut message = SessionMessage::text(message_type, timestamp, content);
            message.metadata = metadata;
            message.model = model;
            return Some(message);
        }

        if kind == "assistant" {
            let content_value = record.pointer("/message/content");
            if let Some(items) = content_value.and_then(Value::as_array) {
                let thinking = Self::text(content_value, &["thinking"]);
                let text = Self::text(content_value, &["text"]);
                if let Some(tool) = items
                    .iter()
                    .find(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"))
                {
                    let mut message = SessionMessage::text(MessageType::ToolUse, timestamp, text);
                    message.tool_name = Some(
                        tool.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_owned(),
                    );
                    message.tool_input = tool
                        .get("input")
                        .and_then(Value::as_object)
                        .cloned()
                        .or_else(|| Some(Map::new()));
                    message.call_id = record
                        .get("uuid")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    message.reasoning_content = (!thinking.is_empty()).then_some(thinking);
                    message.model = model;
                    return Some(message);
                }
                let mut message = SessionMessage::text(MessageType::Assistant, timestamp, text);
                message.reasoning_content = (!thinking.is_empty()).then_some(thinking);
                message.model = model;
                return Some(message);
            }
            let mut message = SessionMessage::text(
                MessageType::Assistant,
                timestamp,
                Self::text(content_value, &[]),
            );
            message.model = model;
            return Some(message);
        }

        if kind == "tool_result" || record.get("toolUseResult").is_some() {
            let output_value = record
                .pointer("/message/content")
                .or_else(|| record.get("toolUseResult"));
            let output = Self::text(output_value, &["text"]);
            let tool_name = output_value
                .and_then(Value::as_array)
                .and_then(|items| {
                    items.iter().find_map(|item| {
                        item.get("tool_use_id")
                            .and_then(Value::as_str)
                            .and_then(|id| id.split(':').next())
                            .map(str::to_owned)
                    })
                })
                .unwrap_or_else(|| "tool".into());
            let mut message = SessionMessage::text(
                MessageType::ToolResult,
                timestamp,
                output.chars().take(300).collect::<String>(),
            );
            message.tool_name = Some(tool_name);
            message.tool_output = Some(ToolOutput {
                output: Some(output),
                preview: None,
                truncated: false,
                extra: Map::new(),
            });
            message.call_id = record
                .get("sourceToolAssistantUUID")
                .and_then(Value::as_str)
                .map(str::to_owned);
            message.model = model;
            return Some(message);
        }
        None
    }

    fn merge_assistant(messages: Vec<SessionMessage>) -> Vec<SessionMessage> {
        let mut merged = Vec::new();
        let mut thinking: Option<SessionMessage> = None;
        for mut message in messages {
            if message.message_type == MessageType::Assistant
                && message.content.as_deref().unwrap_or_default().is_empty()
                && message.reasoning_content.is_some()
            {
                thinking = Some(message);
            } else if message.message_type == MessageType::Assistant
                && message
                    .content
                    .as_deref()
                    .is_some_and(|text| !text.is_empty())
                && thinking.is_some()
            {
                message.reasoning_content =
                    thinking.take().and_then(|value| value.reasoning_content);
                merged.push(message);
            } else {
                if let Some(pending) = thinking.take() {
                    merged.push(pending)
                }
                merged.push(message);
            }
        }
        if let Some(pending) = thinking {
            merged.push(pending)
        }
        merged
    }

    fn file_times(path: &Path) -> (i64, i64) {
        let Ok(metadata) = fs::metadata(path) else {
            return (0, 0);
        };
        let millis = |time: std::io::Result<std::time::SystemTime>| {
            time.ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_millis() as i64)
                .unwrap_or(0)
        };
        (millis(metadata.created()), millis(metadata.modified()))
    }

    fn summary(&self, session_id: &str, entry: Option<&HistoryEntry>) -> Option<Session> {
        // Bound list I/O independently of transcript size. Like the other JSONL
        // providers, the message count is a prefix count until detail is opened.
        const SUMMARY_BYTES: u64 = 256 * 1024;
        let file_path = if let Some(entry) = entry {
            self.projects_path()
                .join(entry.project.as_ref()?.replace('/', "-"))
                .join(format!("{session_id}.jsonl"))
        } else {
            self.transcripts_path().join(format!("{session_id}.jsonl"))
        };
        let canonical = file_path.canonicalize().ok()?;
        if !canonical.starts_with(self.root.canonicalize().ok()?) {
            return None;
        }
        let mut bytes = Vec::new();
        fs::File::open(&canonical)
            .ok()?
            .take(SUMMARY_BYTES)
            .read_to_end(&mut bytes)
            .ok()?;
        let source = String::from_utf8_lossy(&bytes);
        let mut first = String::new();
        let mut last = String::new();
        let mut count = 0;
        for record in source
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        {
            let kind = record.get("type").and_then(Value::as_str);
            if !matches!(
                kind,
                Some("user" | "assistant" | "tool_use" | "tool_result" | "system")
            ) {
                continue;
            }
            count += 1;
            let text = Self::text(
                record
                    .pointer("/message/content")
                    .or_else(|| record.get("content")),
                &["text"],
            );
            if !text.trim().is_empty() {
                let preview = text.chars().take(100).collect::<String>();
                if first.is_empty() {
                    first = preview.clone();
                }
                last = preview;
            }
        }
        let (created_at, updated_at) = Self::file_times(&canonical);
        Some(Session {
            id: session_id.into(),
            app_type: AppType::Claude,
            file_name: format!("{session_id}.jsonl"),
            file_path,
            created_at,
            updated_at: updated_at.max(entry.map_or(0, |entry| entry.timestamp)),
            message_count: count,
            first_message: if first.is_empty() {
                entry
                    .and_then(|entry| entry.display.clone())
                    .unwrap_or_default()
            } else {
                first
            },
            last_message: last,
            directory: entry
                .and_then(|entry| entry.project.as_ref())
                .map(PathBuf::from),
            uuid: None,
            kind: SessionKind::Main,
            parent_session_id: None,
            agent_type: None,
        })
    }

    fn new_detail(&self, entry: &HistoryEntry) -> Option<SessionDetail> {
        let project = entry.project.as_ref()?;
        let file_path = self
            .projects_path()
            .join(project.replace('/', "-"))
            .join(format!("{}.jsonl", entry.session_id));
        if !file_path.exists() {
            return None;
        }
        let records = Self::read_values(&file_path);
        let mut current_model = None;
        let mut messages = Vec::new();
        for record in &records {
            current_model = Self::message_model(record, current_model);
            if let Some(message) = Self::parse_new_message(record, current_model.clone()) {
                messages.push(message)
            }
        }
        let mut inherited_model = None;
        for message in messages.iter_mut().rev() {
            if message.model.is_some() {
                inherited_model = message.model.clone()
            } else {
                message.model = inherited_model.clone()
            }
        }
        let messages = Self::merge_assistant(messages);
        let first = messages
            .first()
            .and_then(|message| message.content.as_deref())
            .unwrap_or_default()
            .chars()
            .take(100)
            .collect();
        let last = messages
            .last()
            .and_then(|message| message.content.as_deref())
            .unwrap_or_default()
            .chars()
            .take(100)
            .collect();
        Some(SessionDetail {
            session: Session {
                id: entry.session_id.clone(),
                app_type: AppType::Claude,
                file_name: format!("{}.jsonl", entry.session_id),
                file_path,
                created_at: entry.timestamp,
                updated_at: entry.timestamp,
                message_count: messages.len(),
                first_message: first,
                last_message: last,
                directory: Some(PathBuf::from(project)),
                uuid: None,
                kind: SessionKind::Main,
                parent_session_id: None,
                agent_type: None,
            },
            messages,
        })
    }

    fn old_detail(&self, session_id: &str) -> Option<SessionDetail> {
        let file_path = self.transcripts_path().join(format!("{session_id}.jsonl"));
        if !file_path.exists() {
            return None;
        }
        let mut messages = Vec::new();
        for record in Self::read_values(&file_path) {
            let kind = match record.get("type").and_then(Value::as_str) {
                Some("user") => MessageType::User,
                Some("assistant") => MessageType::Assistant,
                Some("tool_use") => MessageType::ToolUse,
                Some("tool_result") => MessageType::ToolResult,
                Some("system") => MessageType::System,
                _ => continue,
            };
            let content = record
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if kind == MessageType::User && content.is_empty() {
                continue;
            }
            let mut message = SessionMessage::text(
                kind,
                record
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                content,
            );
            message.reasoning_content = record
                .get("reasoning_content")
                .and_then(Value::as_str)
                .map(str::to_owned);
            message.tool_name = record
                .get("tool_name")
                .and_then(Value::as_str)
                .map(str::to_owned);
            message.tool_input = record.get("tool_input").and_then(Value::as_object).cloned();
            message.call_id = record
                .get("callId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            message.model = Self::model(record.get("model"));
            messages.push(message);
        }
        let (created_at, updated_at) = Self::file_times(&file_path);
        let first = messages
            .first()
            .and_then(|message| message.content.as_deref())
            .unwrap_or_default()
            .chars()
            .take(100)
            .collect();
        let last = messages
            .last()
            .and_then(|message| message.content.as_deref())
            .unwrap_or_default()
            .chars()
            .take(100)
            .collect();
        Some(SessionDetail {
            session: Session {
                id: session_id.into(),
                app_type: AppType::Claude,
                file_name: format!("{session_id}.jsonl"),
                file_path,
                created_at,
                updated_at,
                message_count: messages.len(),
                first_message: first,
                last_message: last,
                directory: None,
                uuid: None,
                kind: SessionKind::Main,
                parent_session_id: None,
                agent_type: None,
            },
            messages,
        })
    }
}

impl SessionProvider for ClaudeProvider {
    fn app_type(&self) -> AppType {
        AppType::Claude
    }
    fn is_available(&self) -> bool {
        self.projects_path().exists() || self.transcripts_path().exists()
    }

    fn sessions(&self) -> Result<Vec<Session>> {
        let mut sessions = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for entry in self.history().into_iter().rev() {
            if !seen.insert(entry.session_id.clone()) {
                continue;
            }
            if let Some(session) = self.summary(&entry.session_id, Some(&entry)) {
                sessions.push(session);
            }
        }
        if let Ok(entries) = fs::read_dir(self.transcripts_path()) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with("ses_") || !name.ends_with(".jsonl") {
                    continue;
                }
                if let Some(session) = self.summary(name.trim_end_matches(".jsonl"), None) {
                    sessions.push(session)
                }
            }
        }
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        Ok(sessions)
    }

    fn session_detail(&self, session_id: &str) -> Result<Option<SessionDetail>> {
        if session_id.contains('-') {
            Ok(self
                .history()
                .iter()
                .find(|entry| entry.session_id == session_id)
                .and_then(|entry| self.new_detail(entry)))
        } else {
            Ok(self.old_detail(session_id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_reads_only_a_bounded_prefix_for_both_transcript_formats() {
        let root =
            std::env::temp_dir().join(format!("yes-claude-summary-test-{}", std::process::id()));
        let project = root.join("projects/-tmp-project");
        let transcripts = root.join("transcripts");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&transcripts).unwrap();
        let modern = |text: &str| {
            json!({"type": "user", "promptId": text, "message": {"content": text}}).to_string()
        };
        let legacy = |text: &str| json!({"type": "user", "content": text}).to_string();
        for (path, first, last) in [
            (
                project.join("test-modern.jsonl"),
                modern("first"),
                modern("tail"),
            ),
            (
                transcripts.join("ses_legacy.jsonl"),
                legacy("first"),
                legacy("tail"),
            ),
        ] {
            fs::write(
                path,
                format!("{first}\n{}\n{last}\n", " ".repeat(512 * 1024)),
            )
            .unwrap();
        }
        fs::write(root.join("history.jsonl"), json!({"sessionId":"test-modern", "project":"/tmp/project", "timestamp":1, "display":"history preview"}).to_string()).unwrap();
        let provider = ClaudeProvider::with_root(root.clone());
        let sessions = provider.sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        for session in sessions {
            assert_eq!(session.message_count, 1);
            assert_eq!(session.first_message, "first");
            assert_eq!(session.last_message, "first");
            let detail = provider.session_detail(&session.id).unwrap().unwrap();
            assert_eq!(detail.messages.len(), 2);
            assert_eq!(detail.messages[1].content.as_deref(), Some("tail"));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn merges_thinking_with_following_assistant_text() {
        let mut thinking = SessionMessage::text(MessageType::Assistant, "now", "");
        thinking.reasoning_content = Some("reason".into());
        let text = SessionMessage::text(MessageType::Assistant, "now", "answer");
        let merged = ClaudeProvider::merge_assistant(vec![thinking, text]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].reasoning_content.as_deref(), Some("reason"));
    }
}
