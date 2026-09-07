use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read as _, Seek as _, SeekFrom},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::Result;
use chrono::Utc;
use regex::Regex;
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

impl Default for ClaudeProvider {
    fn default() -> Self {
        Self {
            root: std::env::var_os("CLAUDE_CONFIG_DIR")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".claude")),
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

    // Content blocks, rather than promptId/toolUseResult, determine the message kind.
    fn parse_new_message(record: &Value, model: Option<String>) -> Vec<SessionMessage> {
        let role = record
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(role, "user" | "assistant")
            || record.get("isMeta").and_then(Value::as_bool) == Some(true)
            || record.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || record
                .get("teamName")
                .and_then(Value::as_str)
                .is_some_and(|name| !name.is_empty())
        {
            return vec![];
        }
        let content = record.pointer("/message/content");
        let blocks = match content {
            Some(Value::String(text)) => vec![json!({"type":"text", "text":text})],
            Some(Value::Array(blocks)) => blocks.clone(),
            _ => return vec![],
        };
        let mut messages = Vec::new();
        for block in blocks {
            let kind = block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut message = SessionMessage::text(
                if role == "user" {
                    MessageType::User
                } else {
                    MessageType::Assistant
                },
                Self::timestamp(record),
                "",
            );
            match kind {
                "text" => {
                    let text = block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if text.trim().is_empty() {
                        continue;
                    }
                    message.content = Some(text.to_owned());
                    if role == "user" {
                        for (tag, subtype) in [
                            ("local-command-caveat", "caveat"),
                            ("local-command-stdout", "command_output"),
                            ("command-name", "command"),
                        ] {
                            if text.trim_start().starts_with(&format!("<{tag}>")) {
                                message.message_type = MessageType::System;
                                message.content = Self::capture(text, tag);
                                if subtype == "command" {
                                    let name = message.content.clone().unwrap_or_default();
                                    let detail =
                                        Self::capture(text, "command-message").unwrap_or_default();
                                    let args =
                                        Self::capture(text, "command-args").unwrap_or_default();
                                    message.content =
                                        Some(format!("{name} {detail} {args}").trim().to_owned());
                                    message.metadata.insert("command".into(), json!(name));
                                }
                                message.metadata.insert("subtype".into(), json!(subtype));
                                break;
                            }
                        }
                    }
                }
                "thinking" | "redacted_thinking" if role == "assistant" => {
                    message.reasoning_content = block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    if kind == "redacted_thinking" {
                        continue;
                    }
                }
                "tool_use" if role == "assistant" => {
                    message.message_type = MessageType::ToolUse;
                    message.tool_name =
                        block.get("name").and_then(Value::as_str).map(str::to_owned);
                    message.call_id = block.get("id").and_then(Value::as_str).map(str::to_owned);
                    message.tool_input = block.get("input").and_then(Value::as_object).cloned();
                }
                "tool_result" if role == "user" => {
                    message.message_type = MessageType::ToolResult;
                    message.call_id = block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let output = Self::text(block.get("content"), &["text"]);
                    message.tool_output = Some(ToolOutput {
                        output: Some(output),
                        preview: None,
                        truncated: false,
                        extra: Map::new(),
                    });
                    if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                        message.metadata.insert("subtype".into(), json!("error"));
                    }
                }
                _ => continue,
            }
            message.model = model.clone();
            messages.push(message);
        }
        messages
    }

    // Follow the current branch like the official SDK. logicalParentUuid is
    // deliberately not followed across compaction boundaries.
    fn conversation_chain(records: &[Value]) -> Vec<&Value> {
        let entries = records
            .iter()
            .filter(|record| {
                matches!(
                    record.get("type").and_then(Value::as_str),
                    Some("user" | "assistant" | "system" | "progress" | "attachment")
                ) && record.get("uuid").and_then(Value::as_str).is_some()
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return records.iter().collect();
        }
        let by_id = entries
            .iter()
            .enumerate()
            .map(|(index, record)| (record["uuid"].as_str().unwrap(), index))
            .collect::<HashMap<_, _>>();
        let parents = entries
            .iter()
            .filter_map(|record| record.get("parentUuid").and_then(Value::as_str))
            .collect::<HashSet<_>>();
        let mut leaves = Vec::new();
        for (index, record) in entries.iter().enumerate() {
            if parents.contains(record["uuid"].as_str().unwrap()) {
                continue;
            }
            let mut current = Some(index);
            let mut seen = HashSet::new();
            while let Some(index) = current {
                if !seen.insert(index) {
                    break;
                }
                let record = entries[index];
                if matches!(record["type"].as_str(), Some("user" | "assistant")) {
                    leaves.push(index);
                    break;
                }
                current = record
                    .get("parentUuid")
                    .and_then(Value::as_str)
                    .and_then(|id| by_id.get(id))
                    .copied();
            }
        }
        let main = leaves
            .iter()
            .copied()
            .filter(|index| {
                let record = entries[*index];
                record.get("isSidechain").and_then(Value::as_bool) != Some(true)
                    && record.get("isMeta").and_then(Value::as_bool) != Some(true)
                    && !record
                        .get("teamName")
                        .and_then(Value::as_str)
                        .is_some_and(|name| !name.is_empty())
            })
            .max();
        let mut current = main.or_else(|| leaves.into_iter().max());
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        while let Some(index) = current {
            if !seen.insert(index) {
                break;
            }
            let record = entries[index];
            chain.push(record);
            current = record
                .get("parentUuid")
                .and_then(Value::as_str)
                .and_then(|id| by_id.get(id))
                .copied();
        }
        chain.reverse();
        chain
    }

    fn project_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let Ok(projects) = fs::read_dir(self.projects_path()) else {
            return files;
        };
        for project in projects.flatten() {
            let Ok(entries) = fs::read_dir(project.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(id) = path.file_stem().and_then(|name| name.to_str()) else {
                    continue;
                };
                if path.extension().is_some_and(|ext| ext == "jsonl") && Self::valid_session_id(id)
                {
                    if let Ok(path) = path.canonicalize() {
                        if self
                            .root
                            .canonicalize()
                            .is_ok_and(|root| path.starts_with(root))
                        {
                            files.push(path);
                        }
                    }
                }
            }
        }
        files.sort();
        files
    }

    fn child_identity(path: &Path) -> Option<(String, String)> {
        if path.parent()?.file_name()? != "subagents" {
            return None;
        }
        let parent = path.parent()?.parent()?.file_name()?.to_str()?;
        let agent = path.file_stem()?.to_str()?.strip_prefix("agent-")?;
        (Self::valid_session_id(parent)
            && !agent.is_empty()
            && agent
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .then(|| (parent.to_owned(), agent.to_owned()))
    }

    fn child_files(&self, parent: &Path) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(parent.with_extension("").join("subagents")) else {
            return Vec::new();
        };
        let Ok(root) = self.root.canonicalize() else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension()? != "jsonl" || Self::child_identity(&path).is_none() {
                    return None;
                }
                let canonical = path.canonicalize().ok()?;
                (canonical.starts_with(&root) && canonical == path).then_some(path)
            })
            .collect()
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

    fn valid_session_id(id: &str) -> bool {
        id.len() == 36
            && id.bytes().enumerate().all(|(i, byte)| {
                if [8, 13, 18, 23].contains(&i) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
    }

    fn project_summary(&self, path: &Path) -> Option<Session> {
        let mut file = fs::File::open(path).ok()?;
        let mut head = Vec::new();
        (&mut file).take(256 * 1024).read_to_end(&mut head).ok()?;
        let mut records = head
            .split(|byte| *byte == b'\n')
            .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
            .collect::<Vec<_>>();
        let child = Self::child_identity(path);
        if child.is_some() {
            for record in &mut records {
                record["isSidechain"] = json!(false);
            }
        }
        if records
            .first()
            .is_some_and(|record| record.get("isSidechain").and_then(Value::as_bool) == Some(true))
        {
            return None;
        }
        let first = records
            .iter()
            .filter(|record| record.get("isCompactSummary").and_then(Value::as_bool) != Some(true))
            .flat_map(|record| Self::parse_new_message(record, None))
            .find(|message| {
                message.message_type == MessageType::User
                    && ![
                        "<session-start-hook>",
                        "<tick>",
                        "<goal>",
                        "[Request interrupted by user",
                    ]
                    .iter()
                    .any(|prefix| {
                        message
                            .content
                            .as_deref()
                            .unwrap_or_default()
                            .trim_start()
                            .starts_with(prefix)
                    })
            })
            .and_then(|message| message.content)
            .unwrap_or_default();
        let directory = records
            .iter()
            .find_map(|record| record.get("cwd").and_then(Value::as_str))
            .map(PathBuf::from);
        let count = records
            .iter()
            .filter(|record| {
                matches!(
                    record.get("type").and_then(Value::as_str),
                    Some("user" | "assistant")
                )
            })
            .count();
        let size = file.metadata().ok()?.len();
        if size > 256 * 1024 {
            file.seek(SeekFrom::Start(size.saturating_sub(256 * 1024)))
                .ok()?;
            let mut tail = Vec::new();
            file.take(256 * 1024).read_to_end(&mut tail).ok()?;
            records.extend(
                tail.split(|byte| *byte == b'\n')
                    .skip(1)
                    .filter_map(|line| serde_json::from_slice::<Value>(line).ok()),
            );
        }
        let title = ["customTitle", "aiTitle", "lastPrompt", "summary"]
            .into_iter()
            .find_map(|key| {
                records.iter().rev().find_map(|record| {
                    record
                        .get(key)
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                })
            })
            .unwrap_or(&first)
            .chars()
            .take(200)
            .collect::<String>();
        if title.trim().is_empty() {
            return None;
        }
        let (created_at, updated_at) = Self::file_times(path);
        Some(Session {
            id: child
                .as_ref()
                .map(|(parent, agent)| format!("{parent}:{agent}"))
                .unwrap_or(path.file_stem()?.to_str()?.to_owned()),
            app_type: AppType::Claude,
            file_name: path.file_name()?.to_str()?.to_owned(),
            file_path: path.to_path_buf(),
            created_at,
            updated_at,
            message_count: count,
            first_message: title,
            last_message: first.clone(),
            directory,
            uuid: None,
            kind: if child.is_some() {
                SessionKind::Subagent
            } else {
                SessionKind::Main
            },
            parent_session_id: child.map(|(parent, _)| parent),
            agent_type: None,
        })
    }

    fn summary(&self, session_id: &str) -> Option<Session> {
        // Bound list I/O independently of transcript size. Like the other JSONL
        // providers, the message count is a prefix count until detail is opened.
        const SUMMARY_BYTES: u64 = 256 * 1024;
        let file_path = self.transcripts_path().join(format!("{session_id}.jsonl"));
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
            updated_at,
            message_count: count,
            first_message: first,
            last_message: last,
            directory: None,
            uuid: None,
            kind: SessionKind::Main,
            parent_session_id: None,
            agent_type: None,
        })
    }

    fn new_detail(&self, file_path: &Path) -> Option<SessionDetail> {
        let mut records = Self::read_values(file_path);
        let mut session = self.project_summary(file_path)?;
        if session.kind == SessionKind::Subagent {
            for record in &mut records {
                record["isSidechain"] = json!(false);
            }
        }
        let mut current_model = None;
        let mut messages = Vec::new();
        for record in Self::conversation_chain(&records) {
            current_model = Self::message_model(record, current_model);
            messages.extend(Self::parse_new_message(record, current_model.clone()));
        }
        let mut inherited_model = None;
        for message in messages.iter_mut().rev() {
            if message.model.is_some() {
                inherited_model = message.model.clone()
            } else {
                message.model = inherited_model.clone()
            }
        }
        let tool_names = messages
            .iter()
            .filter(|message| message.message_type == MessageType::ToolUse)
            .filter_map(|message| Some((message.call_id.clone()?, message.tool_name.clone()?)))
            .collect::<HashMap<_, _>>();
        for message in &mut messages {
            if message.message_type == MessageType::ToolResult {
                message.tool_name = message
                    .call_id
                    .as_ref()
                    .and_then(|id| tool_names.get(id))
                    .cloned();
            }
        }
        self.link_children(file_path, &records, &mut messages);
        session.message_count = messages.len();
        session.last_message = messages
            .last()
            .and_then(|message| message.content.clone())
            .unwrap_or_default();
        Some(SessionDetail { session, messages })
    }

    fn link_children(&self, path: &Path, records: &[Value], messages: &mut Vec<SessionMessage>) {
        let children = self
            .child_files(path)
            .into_iter()
            .filter_map(|path| {
                let (parent, agent) = Self::child_identity(&path)?;
                Some((agent.clone(), format!("{parent}:{agent}")))
            })
            .collect::<HashMap<_, _>>();
        let mut links = HashMap::new();
        for record in Self::conversation_chain(records) {
            let Some(result) = record.get("toolUseResult") else {
                continue;
            };
            let Some(agent) = result.get("agentId").and_then(Value::as_str) else {
                continue;
            };
            let Some(blocks) = record.pointer("/message/content").and_then(Value::as_array) else {
                continue;
            };
            // Envelope metadata belongs to a single result, never guess across parallel results.
            let results = blocks
                .iter()
                .filter(|b| b["type"] == "tool_result")
                .collect::<Vec<_>>();
            if results.len() != 1 {
                continue;
            }
            let Some(call) = results[0]["tool_use_id"].as_str() else {
                continue;
            };
            if !messages.iter().any(|m| {
                m.call_id.as_deref() == Some(call)
                    && matches!(m.tool_name.as_deref(), Some("Agent" | "Task"))
            }) {
                continue;
            }
            links.insert(call.to_owned(), agent.to_owned());
            for message in messages
                .iter_mut()
                .filter(|m| m.call_id.as_deref() == Some(call))
            {
                message.sub_agent_session_id = children.get(agent).cloned();
                if let Some(model) = result.get("resolvedModel").and_then(Value::as_str) {
                    message.model = Some(model.to_owned());
                }
                if message.message_type == MessageType::ToolResult
                    && message.metadata.get("subtype").and_then(Value::as_str) != Some("error")
                {
                    let asynchronous = result.get("isAsync").and_then(Value::as_bool) == Some(true)
                        || result["status"] == "async_launched";
                    message.metadata.insert(
                        "subtype".into(),
                        json!(if asynchronous { "running" } else { "completed" }),
                    );
                    if asynchronous {
                        message.tool_output = None;
                    }
                }
            }
        }
        let mut consumed = HashSet::new();
        for (index, notification) in messages.iter().enumerate() {
            let Some(text) = notification
                .content
                .as_deref()
                .filter(|text| text.trim_start().starts_with("<task-notification>"))
            else {
                continue;
            };
            let Some(call) = Self::capture(text, "tool-use-id") else {
                continue;
            };
            let Some(agent) = Self::capture(text, "task-id") else {
                continue;
            };
            if links.get(&call) != Some(&agent) {
                continue;
            }
            consumed.insert(index);
        }
        let notifications = messages
            .iter()
            .enumerate()
            .filter(|(index, _)| consumed.contains(index))
            .map(|(_, message)| message.content.clone().unwrap())
            .collect::<Vec<_>>();
        for text in notifications {
            let call = Self::capture(&text, "tool-use-id").unwrap();
            if let Some(message) = messages.iter_mut().find(|m| {
                m.message_type == MessageType::ToolResult && m.call_id.as_deref() == Some(&call)
            }) {
                message.metadata.insert(
                    "subtype".into(),
                    json!(Self::capture(&text, "status").unwrap_or_else(|| "running".into())),
                );
                message.tool_output = Some(ToolOutput {
                    output: Self::capture(&text, "result")
                        .or_else(|| Self::capture(&text, "summary")),
                    preview: None,
                    truncated: false,
                    extra: Map::new(),
                });
            }
        }
        let mut index = 0;
        messages.retain(|_| {
            let keep = !consumed.contains(&index);
            index += 1;
            keep
        });
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
            message.tool_output = record
                .get("tool_output")
                .and_then(|output| serde_json::from_value(output.clone()).ok());
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
        let main_files = self.project_files();
        let files = main_files
            .iter()
            .cloned()
            .chain(main_files.iter().flat_map(|path| self.child_files(path)));
        for path in files {
            if let Some(session) = self.project_summary(&path) {
                if seen.insert(session.id.clone()) {
                    sessions.push(session);
                }
            }
        }
        if let Ok(entries) = fs::read_dir(self.transcripts_path()) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with("ses_") || !name.ends_with(".jsonl") {
                    continue;
                }
                if let Some(session) = self.summary(name.trim_end_matches(".jsonl")) {
                    sessions.push(session)
                }
            }
        }
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        Ok(sessions)
    }

    fn session_detail(&self, session_id: &str) -> Result<Option<SessionDetail>> {
        if let Some((parent, agent)) = session_id.split_once(':') {
            if !Self::valid_session_id(parent) {
                return Ok(None);
            }
            return Ok(self
                .project_files()
                .iter()
                .filter(|path| path.file_stem().and_then(|name| name.to_str()) == Some(parent))
                .flat_map(|path| self.child_files(path))
                .find(|path| Self::child_identity(path).is_some_and(|(_, id)| id == agent))
                .and_then(|path| self.new_detail(&path)));
        }
        if Self::valid_session_id(session_id) {
            Ok(self
                .project_files()
                .iter()
                .find(|path| path.file_stem().and_then(|name| name.to_str()) == Some(session_id))
                .and_then(|path| self.new_detail(path)))
        } else if session_id.starts_with("ses_") && !session_id.contains(['/', '\\']) {
            Ok(self.old_detail(session_id))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_sidechain_details_and_links_async_notifications() {
        let root = std::env::temp_dir().join(format!("yes-claude-agents-{}", std::process::id()));
        let project = root.join("projects/project");
        let parent = "33333333-3333-4333-8333-333333333333";
        let children = project.join(parent).join("subagents");
        fs::create_dir_all(&children).unwrap();
        let write = |path: PathBuf, records: Vec<Value>| {
            fs::write(
                path,
                records
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .unwrap()
        };
        write(
            project.join(format!("{parent}.jsonl")),
            vec![
                json!({"type":"user","message":{"content":"Inspect directories"}}),
                json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Agent","id":"call-a","input":{"description":"Inspect videos"}},{"type":"tool_use","name":"Agent","id":"call-b","input":{"description":"Inspect scripts"}}]}}),
                json!({"type":"user","toolUseResult":{"agentId":"aaa","isAsync":true,"status":"async_launched","resolvedModel":"gateway-model"},"message":{"content":[{"type":"tool_result","tool_use_id":"call-a","content":"Internal launch metadata"}]}}),
                json!({"type":"user","toolUseResult":{"agentId":"bbb","isAsync":true},"message":{"content":[{"type":"tool_result","tool_use_id":"call-b","content":"Internal launch metadata"}]}}),
                json!({"type":"user","message":{"content":"<task-notification><task-id>bbb</task-id><tool-use-id>call-b</tool-use-id><status>completed</status><result>Scripts summary</result></task-notification>"}}),
            ],
        );
        for (agent, prompt) in [("aaa", "Analyze videos"), ("bbb", "Analyze scripts")] {
            write(
                children.join(format!("agent-{agent}.jsonl")),
                vec![
                    json!({"type":"user","uuid":"u","parentUuid":null,"isSidechain":true,"sessionId":parent,"agentId":agent,"message":{"content":prompt}}),
                    json!({"type":"assistant","uuid":"a","parentUuid":"u","isSidechain":true,"message":{"model":"gateway-model","content":[{"type":"text","text":"Analysis"}]}}),
                ],
            );
        }
        let provider = ClaudeProvider::with_root(root.clone());
        let sessions = provider.sessions().unwrap();
        assert_eq!(sessions.len(), 3);
        let child_id = format!("{parent}:aaa");
        let child = sessions.iter().find(|s| s.id == child_id).unwrap();
        assert_eq!(child.kind, SessionKind::Subagent);
        assert_eq!(child.parent_session_id.as_deref(), Some(parent));
        assert_eq!(child.first_message, "Analyze videos");
        let detail = provider.session_detail(&child_id).unwrap().unwrap();
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[1].content.as_deref(), Some("Analysis"));
        let main = provider.session_detail(parent).unwrap().unwrap();
        assert_eq!(main.messages.len(), 5);
        for message in main
            .messages
            .iter()
            .filter(|m| m.call_id.as_deref() == Some("call-a"))
        {
            assert_eq!(
                message.sub_agent_session_id.as_deref(),
                Some(child_id.as_str())
            );
        }
        let results = main
            .messages
            .iter()
            .filter(|m| m.message_type == MessageType::ToolResult)
            .collect::<Vec<_>>();
        assert_eq!(results[0].metadata["subtype"], "running");
        assert!(results[0].tool_output.is_none());
        assert_eq!(results[1].metadata["subtype"], "completed");
        assert_eq!(
            results[1].tool_output.as_ref().unwrap().output.as_deref(),
            Some("Scripts summary")
        );
        fs::remove_file(children.join("agent-aaa.jsonl")).unwrap();
        assert!(provider.session_detail(&child_id).unwrap().is_none());
        assert!(
            provider
                .session_detail(parent)
                .unwrap()
                .unwrap()
                .messages
                .iter()
                .filter(|m| m.call_id.as_deref() == Some("call-a"))
                .all(|m| m.sub_agent_session_id.is_none())
        );
        assert!(
            provider
                .session_detail(&format!("{parent}:../../escape"))
                .unwrap()
                .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }

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
                project.join("11111111-1111-4111-8111-111111111111.jsonl"),
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
    fn parses_ordered_blocks_without_prompt_id_and_preserves_results() {
        let user = json!({"type":"user","message":{"content":"Explain this"}});
        assert_eq!(
            ClaudeProvider::parse_new_message(&user, None)[0].message_type,
            MessageType::User
        );
        let command = json!({"type":"user","message":{"content":"<command-name>/review</command-name><command-message>Review changes</command-message><command-args>HEAD~1</command-args>"}});
        let parsed = ClaudeProvider::parse_new_message(&command, None);
        assert_eq!(parsed[0].message_type, MessageType::System);
        assert_eq!(
            parsed[0].content.as_deref(),
            Some("/review Review changes HEAD~1")
        );
        assert_eq!(parsed[0].metadata["command"], "/review");
        let assistant = json!({"type":"assistant","uuid":"envelope","message":{"content":[
            {"type":"thinking","thinking":"Plan"}, {"type":"text","text":"Before"},
            {"type":"tool_use","id":"one","name":"Read","input":{"file_path":"a"}},
            {"type":"tool_use","id":"two","name":"Bash","input":{"command":"pwd"}},
            {"type":"text","text":"After"}
        ]}});
        let messages = ClaudeProvider::parse_new_message(&assistant, None);
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0].reasoning_content.as_deref(), Some("Plan"));
        assert_eq!(messages[1].content.as_deref(), Some("Before"));
        assert_eq!(messages[2].call_id.as_deref(), Some("one"));
        assert_eq!(messages[3].call_id.as_deref(), Some("two"));
        assert_eq!(messages[4].content.as_deref(), Some("After"));
        let result = json!({"type":"user","toolUseResult":{"stdout":"not the content source"},"message":{"content":[
            {"type":"tool_result","tool_use_id":"two","content":[{"type":"text","text":"Error details"}],"is_error":true},
            {"type":"tool_result","tool_use_id":"one","content":"File contents"}
        ]}});
        let messages = ClaudeProvider::parse_new_message(&result, None);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].call_id.as_deref(), Some("two"));
        assert_eq!(messages[0].metadata["subtype"], "error");
        assert_eq!(
            messages[0].tool_output.as_ref().unwrap().output.as_deref(),
            Some("Error details")
        );
        assert_eq!(
            messages[1].tool_output.as_ref().unwrap().output.as_deref(),
            Some("File contents")
        );
    }

    #[test]
    fn follows_latest_main_branch_and_stops_at_compaction() {
        let records = vec![
            json!({"type":"user","uuid":"root","parentUuid":null}),
            json!({"type":"assistant","uuid":"old","parentUuid":"root"}),
            json!({"type":"assistant","uuid":"new","parentUuid":"root"}),
            json!({"type":"progress","uuid":"progress","parentUuid":"new"}),
            json!({"type":"assistant","uuid":"side","parentUuid":"root","isSidechain":true}),
        ];
        let chain = ClaudeProvider::conversation_chain(&records);
        assert_eq!(
            chain
                .iter()
                .map(|r| r["uuid"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["root", "new"]
        );
        let mut compacted = records;
        compacted.extend([
            json!({"type":"system","uuid":"boundary","parentUuid":null,"logicalParentUuid":"new"}),
            json!({"type":"user","uuid":"summary","parentUuid":"boundary","isCompactSummary":true,"message":{"content":"Summary"}}),
            json!({"type":"assistant","uuid":"answer","parentUuid":"summary"}),
        ]);
        let chain = ClaudeProvider::conversation_chain(&compacted);
        assert_eq!(
            chain
                .iter()
                .map(|r| r["uuid"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["boundary", "summary", "answer"]
        );
        assert_eq!(ClaudeProvider::parse_new_message(chain[1], None).len(), 1);
    }

    #[test]
    fn discovers_sessions_without_history_and_uses_saved_title() {
        let root =
            std::env::temp_dir().join(format!("yes-claude-discovery-{}", std::process::id()));
        let project = root.join("projects/custom-project-name");
        fs::create_dir_all(&project).unwrap();
        let id = "22222222-2222-4222-8222-222222222222";
        let records = [
            json!({"type":"user","uuid":"u","cwd":"/tmp/a.b_c","message":{"content":"[Bug] actual request"}}),
            json!({"type":"assistant","uuid":"a","parentUuid":"u","message":{"content":[{"type":"text","text":"Answer"}]}}),
            json!({"type":"custom-title","customTitle":"Saved name"}),
        ];
        fs::write(
            project.join(format!("{id}.jsonl")),
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let provider = ClaudeProvider::with_root(root.clone());
        let list = provider.sessions().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].first_message, "Saved name");
        assert_eq!(list[0].directory.as_deref(), Some(Path::new("/tmp/a.b_c")));
        let detail = provider.session_detail(id).unwrap().unwrap();
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.session.first_message, list[0].first_message);
        assert!(provider.session_detail("../../escape").unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
