use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use walkdir::WalkDir;

use crate::{
    AppType, MessageType, Session, SessionDetail, SessionMessage,
    model::{SessionKind, TokenUsage, ToolOutput},
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

    fn file_metadata(path: &Path) -> Option<Value> {
        let file = fs::File::open(path).ok()?;
        let mut line = String::new();
        BufReader::new(file.take(256 * 1024))
            .read_line(&mut line)
            .ok()?;
        let record: Value = serde_json::from_str(&line).ok()?;
        (record.get("type").and_then(Value::as_str) == Some("session_meta")).then_some(record)
    }

    fn read_session_history(
        path: &Path,
        files: &[(PathBuf, Value)],
        byte_limit: Option<u64>,
        visited: &mut HashSet<PathBuf>,
    ) -> Result<Vec<Value>> {
        anyhow::ensure!(
            visited.insert(path.to_path_buf()),
            "Cyclic Codex history reference"
        );
        let mut source = String::new();
        fs::File::open(path)?
            .take(byte_limit.unwrap_or(u64::MAX))
            .read_to_string(&mut source)?;
        let records = source
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect::<Vec<_>>();
        let meta = Self::metadata(&records);
        let base = meta.get("history_base");
        let base_id = base
            .and_then(|base| base.get("thread_id"))
            .and_then(Value::as_str);
        let end = base
            .and_then(|base| base.get("end_ordinal_exclusive"))
            .and_then(Value::as_u64);
        // A continuation references an earlier segment of this same logical session.
        let parent = base_id
            .filter(|id| Some(*id) == meta.get("id").and_then(Value::as_str))
            .zip(end)
            .and_then(|(id, end)| {
                files
                    .iter()
                    .filter(|(candidate, record)| {
                        candidate != path
                            && record.pointer("/payload/id").and_then(Value::as_str) == Some(id)
                            && record.get("ordinal").and_then(Value::as_u64).unwrap_or(0) < end
                            && !visited.contains(candidate)
                    })
                    .max_by_key(|(_, record)| {
                        (
                            record.get("ordinal").and_then(Value::as_u64).unwrap_or(0),
                            record.pointer("/payload/timestamp").and_then(Value::as_str),
                        )
                    })
            });
        let Some((parent_path, _)) = parent else {
            return Ok(records);
        };
        let byte_limit = base
            .and_then(|base| base.get("end_byte_offset"))
            .and_then(Value::as_u64);
        let mut inherited = Self::read_session_history(parent_path, files, byte_limit, visited)?;
        inherited.retain(|record| {
            record.get("type").and_then(Value::as_str) != Some("session_meta")
                && record
                    .get("ordinal")
                    .and_then(Value::as_u64)
                    .is_none_or(|ordinal| Some(ordinal) < end)
        });
        let mut combined = records
            .iter()
            .filter(|record| record.get("type").and_then(Value::as_str) == Some("session_meta"))
            .cloned()
            .collect::<Vec<_>>();
        combined.extend(inherited);
        combined.extend(
            records.into_iter().filter(|record| {
                record.get("type").and_then(Value::as_str) != Some("session_meta")
            }),
        );
        Ok(combined)
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

    fn session_kind(
        meta: &Map<String, Value>,
    ) -> Option<(SessionKind, Option<String>, Option<String>)> {
        let Some(subagent) = meta.get("source").and_then(|source| source.get("subagent")) else {
            return Some((SessionKind::Main, None, None));
        };
        // Guardian/review workers are internal control sessions, not user conversations.
        let spawn = subagent.get("thread_spawn")?;
        let parent = spawn.get("parent_thread_id")?.as_str()?.to_owned();
        let label = spawn
            .get("agent_path")
            .and_then(Value::as_str)
            .and_then(|path| path.rsplit('/').find(|part| !part.is_empty()))
            .or_else(|| spawn.get("agent_role").and_then(Value::as_str))
            .or_else(|| spawn.get("agent_nickname").and_then(Value::as_str))
            .map(str::to_owned);
        Some((SessionKind::Subagent, Some(parent), label))
    }

    fn subagent_content_preview(meta: &Map<String, Value>, records: &[Value]) -> String {
        // Forked records may be stamped with the child's creation time, so only
        // the explicit history boundary can distinguish inherited messages.
        let start = meta
            .get("subagent_history_start_ordinal")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let own = records
            .iter()
            .enumerate()
            .filter(|(index, record)| {
                record
                    .get("ordinal")
                    .and_then(Value::as_u64)
                    .unwrap_or(*index as u64)
                    >= start as u64
            })
            .map(|(_, record)| record)
            .collect::<Vec<_>>();
        for record in &own {
            if record.get("type").and_then(Value::as_str) != Some("response_item") {
                continue;
            }
            let text = Self::value_text(record.pointer("/payload/content"));
            let preview = match record.pointer("/payload/type").and_then(Value::as_str) {
                Some("agent_message") if text.contains("Message Type: NEW_TASK") => text
                    .split_once("Payload:")
                    .map(|(_, text)| text.trim().to_owned())
                    .unwrap_or_default(),
                Some("message")
                    if record.pointer("/payload/role").and_then(Value::as_str) == Some("user") =>
                {
                    crate::attachments::user_preview(&Self::normalize_user(&text))
                }
                _ => String::new(),
            };
            if !preview.trim().is_empty() {
                return Self::truncate(preview.trim(), 200);
            }
        }
        own.iter()
            .find_map(|record| {
                if record.pointer("/payload/type").and_then(Value::as_str) != Some("message")
                    || record.pointer("/payload/role").and_then(Value::as_str) != Some("assistant")
                {
                    return None;
                }
                let text = Self::value_text(record.pointer("/payload/content"));
                let title = text.lines().map(str::trim).find(|line| !line.is_empty())?;
                Some(Self::truncate(title, 200))
            })
            .unwrap_or_default()
    }

    fn subagent_preview(
        &self,
        path: &Path,
        meta: &Map<String, Value>,
        records: &[Value],
    ) -> String {
        let preview = Self::subagent_content_preview(meta, records);
        if !preview.is_empty() {
            return preview;
        }
        // Tail reads remain bounded. Only explicit ordinals can establish that
        // these records belong to the child rather than its inherited history.
        let read_tail = || -> Option<String> {
            let mut file = fs::File::open(path).ok()?;
            let offset = file.metadata().ok()?.len().saturating_sub(64 * 1024);
            file.seek(SeekFrom::Start(offset)).ok()?;
            let mut reader = BufReader::new(file);
            if offset > 0 {
                reader.read_line(&mut String::new()).ok()?;
            }
            let mut tail = String::new();
            reader.take(64 * 1024).read_to_string(&mut tail).ok()?;
            let records = tail
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter(|record| record.get("ordinal").and_then(Value::as_u64).is_some())
                .collect::<Vec<_>>();
            Some(Self::subagent_content_preview(meta, &records))
        };
        read_tail().unwrap_or_default()
    }

    fn first_user_preview(records: &[Value]) -> String {
        records
            .iter()
            .find_map(|record| {
                if record.get("type").and_then(Value::as_str) != Some("response_item")
                    || record.pointer("/payload/type").and_then(Value::as_str) != Some("message")
                    || record.pointer("/payload/role").and_then(Value::as_str) != Some("user")
                {
                    return None;
                }
                let text = crate::attachments::user_preview(&Self::normalize_user(
                    &Self::value_text(record.pointer("/payload/content")),
                ));
                (!text.trim().is_empty()).then(|| Self::truncate(&text, 200))
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
        if let Some(rest) = trimmed.strip_prefix("<recommended_plugins>") {
            return rest
                .split_once("</recommended_plugins>")
                .map(|(_, rest)| Self::normalize_user(rest))
                .unwrap_or_default();
        }
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

    fn token_usage(value: Option<&Value>) -> Option<TokenUsage> {
        let value = value?;
        let input_tokens = value.get("input_tokens").and_then(Value::as_u64);
        let output_tokens = value.get("output_tokens").and_then(Value::as_u64);
        let total_tokens = value
            .get("total_tokens")
            .and_then(Value::as_u64)
            .or_else(|| input_tokens?.checked_add(output_tokens?));
        let cache_read_tokens = value.get("cached_input_tokens").and_then(Value::as_u64);
        [input_tokens, output_tokens, total_tokens, cache_read_tokens]
            .into_iter()
            .flatten()
            .any(|count| count > 0)
            .then_some(TokenUsage {
                input_tokens,
                output_tokens,
                total_tokens,
                cache_read_tokens,
            })
    }

    fn usage_delta(current: &TokenUsage, previous: &TokenUsage) -> Option<TokenUsage> {
        let pairs = [
            (current.input_tokens, previous.input_tokens),
            (current.output_tokens, previous.output_tokens),
            (current.total_tokens, previous.total_tokens),
            (current.cache_read_tokens, previous.cache_read_tokens),
        ];
        if pairs
            .iter()
            .any(|(a, b)| a.zip(*b).is_some_and(|(a, b)| a < b))
        {
            return None; // A resumed/reset counter cannot be subtracted across epochs.
        }
        let delta = |a: Option<u64>, b: Option<u64>| a?.checked_sub(b?);
        Some(TokenUsage {
            input_tokens: delta(current.input_tokens, previous.input_tokens),
            output_tokens: delta(current.output_tokens, previous.output_tokens),
            total_tokens: delta(current.total_tokens, previous.total_tokens),
            cache_read_tokens: delta(current.cache_read_tokens, previous.cache_read_tokens),
        })
    }

    fn parse_messages(&self, records: &[Value], cwd: Option<&Path>) -> Vec<SessionMessage> {
        let mut messages: Vec<SessionMessage> = Vec::new();
        let mut previous_total = None;
        let mut seen_responses = HashSet::new();
        let mut last_fallback = None;
        let mut explicit_pending = None;
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
            let explicit_usage = record_type == Some("token_usage_record");
            if explicit_usage
                || (record_type == Some("event_msg")
                    && record.pointer("/payload/type").and_then(Value::as_str)
                        == Some("token_count"))
            {
                if explicit_usage
                    && record
                        .pointer("/payload/response_id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !seen_responses.insert(id.to_owned()))
                {
                    continue;
                }
                let total = Self::token_usage(record.pointer(if explicit_usage {
                    "/payload/thread_token_usage"
                } else {
                    "/payload/info/total_token_usage"
                }));
                if total.is_some() && total == previous_total {
                    if !explicit_usage {
                        explicit_pending = None;
                    }
                    continue; // Repeated snapshots and the legacy mirror of an explicit record.
                }
                let last = Self::token_usage(record.pointer(if explicit_usage {
                    "/payload/usage"
                } else {
                    "/payload/info/last_token_usage"
                }));
                let target = messages
                    .iter()
                    .rposition(|message| {
                        matches!(
                            message.message_type,
                            MessageType::Assistant | MessageType::ToolUse | MessageType::User
                        )
                    })
                    .filter(|index| messages[*index].message_type != MessageType::User);
                if !explicit_usage
                    && last.as_ref().is_some_and(|usage| {
                        explicit_pending.as_ref() == Some(&(target, usage.clone()))
                    })
                {
                    previous_total = total.or(previous_total);
                    explicit_pending = None;
                    continue;
                }
                if explicit_usage {
                    explicit_pending = last.clone().map(|usage| (target, usage));
                }
                let usage = if explicit_usage {
                    last
                } else {
                    total
                        .as_ref()
                        .zip(previous_total.as_ref())
                        .and_then(|(current, previous)| Self::usage_delta(current, previous))
                        .or(last)
                        .or_else(|| total.clone())
                };
                let Some(usage) = usage else {
                    continue;
                };
                // Older last-only events lack response IDs: dedup only against the
                // same visible response, never by token values across different requests.
                let marker = (target, usage.clone());
                if !explicit_usage && total.is_none() && last_fallback.as_ref() == Some(&marker) {
                    continue;
                }
                let stored_usage = usage.clone();
                if total.is_some() {
                    previous_total = total;
                }
                if let Some(index) = target {
                    messages[index].usage = Some(match messages[index].usage.as_ref() {
                        Some(previous) => TokenUsage::aggregate([previous, &usage]).unwrap(),
                        None => usage,
                    });
                } else {
                    let mut message = SessionMessage::text(
                        MessageType::Assistant,
                        Self::timestamp(record.get("timestamp")),
                        "",
                    );
                    message.content = None;
                    message.model = current_model.clone();
                    message.usage = Some(usage);
                    messages.push(message);
                }
                let index = target.unwrap_or(messages.len() - 1);
                if explicit_usage {
                    explicit_pending = Some((Some(index), stored_usage.clone()));
                }
                last_fallback = Some((Some(index), stored_usage));
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
                    let mut message = SessionMessage::text(
                        if role == "user" {
                            MessageType::User
                        } else {
                            MessageType::Assistant
                        },
                        timestamp,
                        content,
                    );
                    crate::attachments::normalize(&mut message, payload.get("content"));
                    if message.content.as_deref().unwrap_or_default().is_empty()
                        && message.attachments.is_empty()
                    {
                        continue;
                    }
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

    fn link_subagents(&self, parent_id: &str, messages: &mut [SessionMessage]) {
        let spawn_calls = messages
            .iter()
            .filter(|message| {
                message.message_type == MessageType::ToolUse
                    && message
                        .tool_name
                        .as_deref()
                        .is_some_and(|name| name.ends_with("spawn_agent"))
            })
            .filter_map(|message| message.call_id.clone())
            .collect::<HashSet<_>>();
        if spawn_calls.is_empty() {
            return;
        }
        // Read only bounded metadata, never each child's full transcript.
        let mut targets = HashMap::<String, Option<String>>::new();
        for path in self.session_files() {
            let Ok(file) = fs::File::open(&path) else {
                continue;
            };
            let mut line = String::new();
            if BufReader::new(file.take(256 * 1024))
                .read_line(&mut line)
                .is_err()
            {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if record.get("type").and_then(Value::as_str) != Some("session_meta") {
                continue;
            }
            let Some(meta) = record.get("payload").and_then(Value::as_object) else {
                continue;
            };
            let Some((SessionKind::Subagent, Some(parent), _)) = Self::session_kind(meta) else {
                continue;
            };
            if parent != parent_id {
                continue;
            }
            let Some(id) = meta.get("id").and_then(Value::as_str) else {
                continue;
            };
            let agent_path = record
                .pointer("/payload/source/subagent/thread_spawn/agent_path")
                .and_then(Value::as_str);
            for alias in std::iter::once(id)
                .chain(agent_path)
                .chain(agent_path.and_then(|path| path.rsplit('/').next()))
            {
                targets
                    .entry(alias.to_owned())
                    .and_modify(|previous| {
                        if previous.as_deref() != Some(id) {
                            *previous = None;
                        }
                    })
                    .or_insert_with(|| Some(id.to_owned()));
            }
        }
        let mut linked_calls = HashMap::new();
        for message in messages
            .iter_mut()
            .filter(|message| message.message_type == MessageType::ToolResult)
        {
            let Some(call_id) = message.call_id.as_ref() else {
                continue;
            };
            if !spawn_calls.contains(call_id) {
                continue;
            }
            let output = message
                .tool_output
                .as_ref()
                .and_then(|output| output.output.as_deref())
                .and_then(|text| serde_json::from_str::<Value>(text).ok());
            let target = ["agent_id", "thread_id", "task_name"]
                .into_iter()
                .filter_map(|key| output.as_ref()?.get(key)?.as_str())
                .find_map(|target| targets.get(target).and_then(Clone::clone));
            if let Some(target) = target {
                message.sub_agent_session_id = Some(target.clone());
                linked_calls.insert(call_id.clone(), target);
            }
        }
        for message in messages
            .iter_mut()
            .filter(|message| message.message_type == MessageType::ToolUse)
        {
            message.sub_agent_session_id = message
                .call_id
                .as_ref()
                .and_then(|id| linked_calls.get(id))
                .cloned();
        }
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
                            crate::attachments::user_preview(&Self::normalize_user(&text))
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
        if source.is_empty() {
            return None;
        }
        let records = source
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect::<Vec<_>>();
        if Self::is_review(&records) {
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
        let (kind, parent_session_id, agent_type) = Self::session_kind(&meta)?;
        let index_entry = index.get(&id);
        let candidates = source
            .lines()
            .filter(|line| Self::line_is_summary_item(line))
            .collect::<Vec<_>>();
        let first = if kind == SessionKind::Subagent {
            self.subagent_preview(path, &meta, &records)
        } else {
            Self::first_user_preview(&records)
        };
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
                .filter(|title| !title.trim().is_empty())
                .or_else(|| (!first.is_empty()).then(|| Self::truncate(&first, 200)))
                .or_else(|| agent_type.clone())
                .unwrap_or_default(),
            last_message: Self::truncate(&last, 200),
            directory: meta.get("cwd").and_then(Value::as_str).map(PathBuf::from),
            uuid: None,
            kind,
            parent_session_id,
            agent_type: meta
                .get("source")
                .and_then(|source| source.pointer("/subagent/thread_spawn/agent_role"))
                .and_then(Value::as_str)
                .map(str::to_owned),
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
        let (kind, parent_session_id, agent_type) = Self::session_kind(&meta)?;
        let id = meta
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| Self::file_id(path))?;
        let cwd = meta.get("cwd").and_then(Value::as_str).map(PathBuf::from);
        let mut messages = include_messages.then(|| {
            let own_records = (kind == SessionKind::Subagent)
                .then(|| {
                    meta.get("subagent_history_start_ordinal")
                        .and_then(Value::as_u64)
                })
                .flatten()
                .map(|boundary| {
                    records
                        .iter()
                        .enumerate()
                        .filter(|(index, record)| {
                            record
                                .get("ordinal")
                                .and_then(Value::as_u64)
                                .unwrap_or(*index as u64)
                                >= boundary
                        })
                        .map(|(_, record)| record.clone())
                        .collect::<Vec<_>>()
                });
            self.parse_messages(own_records.as_deref().unwrap_or(records), cwd.as_deref())
        });
        if let Some(messages) = messages.as_mut() {
            self.link_subagents(&id, messages);
        }
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
                .filter(|title| !title.trim().is_empty())
                .or_else(|| {
                    let preview = if kind == SessionKind::Subagent {
                        self.subagent_preview(path, &meta, records)
                    } else {
                        first.clone()
                    };
                    (!preview.is_empty()).then_some(preview)
                })
                .or_else(|| agent_type.clone())
                .unwrap_or_default(),
            last_message: last,
            directory: cwd,
            uuid: None,
            kind,
            parent_session_id,
            agent_type: meta
                .get("source")
                .and_then(|source| source.pointer("/subagent/thread_spawn/agent_role"))
                .and_then(Value::as_str)
                .map(str::to_owned),
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
        let mut records = self
            .session_files()
            .into_par_iter()
            .filter_map(|path| self.make_session_summary(&path, &index))
            .collect::<Vec<_>>();
        // The same session can have multiple paginated rollout files. The latest
        // segment supplies current metadata; retain the original creation time.
        records.sort_by(|a, b| (a.created_at, &a.file_path).cmp(&(b.created_at, &b.file_path)));
        let mut unique = HashMap::<String, Session>::new();
        for mut session in records {
            if let Some(previous) = unique.remove(&session.id) {
                session.created_at = previous.created_at.min(session.created_at);
                session.updated_at = previous.updated_at.max(session.updated_at);
                if !previous.first_message.is_empty() {
                    session.first_message = previous.first_message;
                }
            }
            unique.insert(session.id.clone(), session);
        }
        let mut sessions = unique.into_values().collect::<Vec<_>>();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        Ok(sessions)
    }

    fn session_detail_for_search(&self, session: &Session) -> Result<Option<SessionDetail>> {
        let path = super::search_path(&self.sessions_path(), &session.file_path)?;
        let records = Self::read_session_history(&path, &[], None, &mut HashSet::new())?;
        let metadata = Self::metadata(&records);
        // Metadata may follow a leading record in older rollouts. Paginated histories
        // need the regular sibling discovery path to preserve its loading semantics.
        if metadata.contains_key("history_base") {
            return self.session_detail(&session.id);
        }
        anyhow::ensure!(
            metadata.get("id").and_then(Value::as_str) == Some(session.id.as_str())
                || (!metadata.contains_key("id")
                    && Self::file_id(&path).as_deref() == Some(session.id.as_str())),
            "Session path does not match its ID"
        );
        // Search only consumes messages, so the cached session supplies its metadata.
        let detail = self
            .make_session(&path, &records, &HashMap::new(), true)
            .map(|(_, messages)| SessionDetail {
                session: session.clone(),
                messages,
            });
        Ok(detail)
    }

    fn session_detail(&self, session_id: &str) -> Result<Option<SessionDetail>> {
        let index = self.load_index();
        // Filename suffixes can identify a segment, not the logical thread.
        let files = self
            .session_files()
            .into_par_iter()
            .filter_map(|path| {
                Self::file_metadata(&path)
                    .or_else(|| {
                        Self::file_id(&path)
                            .map(|id| json!({"type":"session_meta","payload":{"id":id}}))
                    })
                    .map(|record| (path, record))
            })
            .collect::<Vec<_>>();
        let matches = files
            .iter()
            .filter(|(_, record)| {
                record.pointer("/payload/id").and_then(Value::as_str) == Some(session_id)
            })
            .collect::<Vec<_>>();
        let Some((path, _)) = matches.iter().copied().max_by_key(|(path, record)| {
            (
                record.pointer("/payload/timestamp").and_then(Value::as_str),
                path,
            )
        }) else {
            return Ok(None);
        };
        let records = Self::read_session_history(path, &files, None, &mut HashSet::new())?;
        Ok(self
            .make_session(path, &records, &index, true)
            .map(|(mut session, messages)| {
                if let Some(created) = matches
                    .iter()
                    .filter_map(|(_, record)| {
                        Self::timestamp_ms(
                            record.pointer("/payload/timestamp").and_then(Value::as_str),
                        )
                    })
                    .min()
                {
                    session.created_at = created;
                }
                SessionDetail { session, messages }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage_counts(input: u64, output: u64, cached: u64) -> Value {
        json!({"input_tokens":input,"output_tokens":output,"cached_input_tokens":cached,
            "reasoning_output_tokens":output/2,"total_tokens":input+output})
    }

    fn usage_event(total: Value, last: Value) -> Value {
        json!({"type":"event_msg","payload":{"type":"token_count","info":{
            "total_token_usage":total,"last_token_usage":last}}})
    }

    fn usage_reply(text: &str) -> Value {
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":text}})
    }

    #[test]
    fn explicit_usage_and_legacy_mirrors_count_once() {
        let provider = CodexProvider::default();
        let first = usage_counts(100, 20, 80);
        let second = usage_counts(50, 10, 40);
        let total = usage_counts(150, 30, 120);
        let explicit = |id: &str, usage: Value, total: Value| {
            json!({"type":"token_usage_record",
            "payload":{"response_id":id,"usage":usage,"thread_token_usage":total}})
        };
        let a = explicit("a", first.clone(), first.clone());
        let messages = provider.parse_messages(&[
            json!({"type":"response_item","payload":{"type":"function_call","name":"Read","call_id":"tool","arguments":"{}"}}),
            a.clone(), usage_event(first.clone(), first.clone()),
            usage_reply("Done"), explicit("b",second.clone(),total.clone()),
            usage_event(total, second), a,
        ], None);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].usage.as_ref().unwrap().input_tokens, Some(100));
        let total =
            TokenUsage::aggregate(messages.iter().filter_map(|message| message.usage.as_ref()))
                .unwrap();
        assert_eq!(total.input_tokens, Some(150));
        assert_eq!(total.output_tokens, Some(30)); // Reasoning is already part of output.
        assert_eq!(total.total_tokens, Some(180));
        assert_eq!(total.cache_hit_rate(), Some(80.0));
    }

    #[test]
    fn legacy_totals_use_deltas_and_handle_counter_resets() {
        let provider = CodexProvider::default();
        let first = usage_counts(100, 20, 80);
        let second = usage_counts(50, 10, 40);
        let total = usage_counts(150, 30, 120);
        let messages = provider.parse_messages(
            &[
                usage_reply("First"),
                usage_event(first.clone(), first.clone()),
                usage_event(first.clone(), first),
                usage_reply("Second"),
                usage_event(total, second.clone()),
                usage_reply("Reset"),
                usage_event(second.clone(), second),
            ],
            None,
        );
        assert_eq!(
            messages
                .iter()
                .map(|m| m.usage.as_ref().unwrap().input_tokens.unwrap())
                .collect::<Vec<_>>(),
            vec![100, 50, 50]
        );
        assert_eq!(
            TokenUsage::aggregate(messages.iter().filter_map(|m| m.usage.as_ref()))
                .unwrap()
                .total_tokens,
            Some(240)
        );
    }

    #[test]
    fn last_only_events_preserve_equal_cost_requests_and_usage_without_text() {
        let provider = CodexProvider::default();
        let usage = usage_counts(100, 20, 80);
        let event = usage_event(Value::Null, usage.clone());
        let messages = provider.parse_messages(
            &[
                event.clone(),
                event.clone(),
                usage_reply("Next"),
                event.clone(),
                event,
            ],
            None,
        );
        assert_eq!(messages.len(), 2);
        assert_eq!(
            TokenUsage::aggregate(messages.iter().filter_map(|m| m.usage.as_ref()))
                .unwrap()
                .input_tokens,
            Some(200)
        );
        assert!(
            CodexProvider::token_usage(Some(&json!({"input_tokens":0,"output_tokens":0})))
                .is_none()
        );
        let unknown =
            CodexProvider::token_usage(Some(&json!({"input_tokens":100,"output_tokens":10})))
                .unwrap();
        assert_eq!(unknown.cache_hit_rate(), None);
        assert_eq!(unknown.total_tokens, Some(110));
    }

    #[test]
    fn explicit_records_without_totals_do_not_duplicate_legacy_snapshots() {
        let provider = CodexProvider::default();
        let usage = usage_counts(100, 20, 80);
        let messages = provider.parse_messages(
            &[
                usage_reply("First"),
                json!({"type":"token_usage_record","payload":{"response_id":"one","usage":usage}}),
                usage_event(usage.clone(), usage.clone()),
                usage_reply("Next"),
                usage_event(usage_counts(200, 40, 160), usage),
            ],
            None,
        );
        assert_eq!(
            TokenUsage::aggregate(messages.iter().filter_map(|m| m.usage.as_ref()))
                .unwrap()
                .input_tokens,
            Some(200)
        );
    }

    #[test]
    fn paginated_rollouts_list_once_and_load_both_history_segments() {
        let root =
            std::env::temp_dir().join(format!("yes-codex-pagination-{}", std::process::id()));
        fs::create_dir_all(root.join("sessions")).unwrap();
        let provider = CodexProvider::with_root(root.clone());
        let id = "01a079ae-c038-7dc3-8ef6-4b8a95d22112";
        let old = root
            .join("sessions")
            .join(format!("rollout-2026-09-07T10-24-50-{id}.jsonl"));
        let next = root.join("sessions").join(format!(
            "rollout-2026-09-07T22-35-54-{id}_01a07c4c-0e7e-7c43-b4e1-7ca9f719b89c.jsonl"
        ));
        let message = |ordinal, text| json!({"timestamp":"2026-09-07T02:24:50Z","ordinal":ordinal,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}});
        let prefix = [json!({"ordinal":0,"type":"session_meta","payload":{"id":id,"timestamp":"2026-09-07T02:24:50Z","source":"vscode"}}), message(1,"First request"), message(2,"Earlier request")]
            .iter().map(|record| format!("{record}\n")).collect::<String>();
        fs::write(
            &old,
            format!("{prefix}{}\n", message(3, "Discarded branch")),
        )
        .unwrap();
        let latest = json!({"ordinal":3,"type":"session_meta","payload":{"id":id,"timestamp":"2026-09-07T14:35:54Z","source":"vscode","history_mode":"paginated","history_base":{"thread_id":id,"end_ordinal_exclusive":3,"end_byte_offset":prefix.len()}}});
        fs::write(
            &next,
            format!("{latest}\n{}\n", message(4, "Latest request")),
        )
        .unwrap();
        let sessions = provider.sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].file_path, next);
        let detail = provider.session_detail(id).unwrap().unwrap();
        assert_eq!(
            provider
                .session_detail_for_search(&sessions[0])
                .unwrap()
                .unwrap()
                .messages,
            detail.messages
        );
        assert_eq!(detail.session.created_at, sessions[0].created_at);
        assert_eq!(detail.session.file_path, next);
        assert_eq!(
            detail
                .messages
                .iter()
                .filter_map(|message| message.content.as_deref())
                .collect::<Vec<_>>(),
            vec!["First request", "Earlier request", "Latest request"]
        );
        // Preserve the normal loader's fallback behavior if metadata is not first.
        let source = fs::read_to_string(&next).unwrap();
        fs::write(&next, format!("{{}}\n{source}")).unwrap();
        assert_eq!(
            provider
                .session_detail_for_search(&sessions[0])
                .unwrap()
                .unwrap()
                .messages,
            provider.session_detail(id).unwrap().unwrap().messages
        );
        fs::write(&next, source).unwrap();
        assert!(
            provider
                .session_detail("01a07c4c-0e7e-7c43-b4e1-7ca9f719b89c")
                .unwrap()
                .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sidebar_classifies_workers_and_uses_real_user_titles() {
        let root = std::env::temp_dir().join(format!("yes-codex-sidebar-{}", std::process::id()));
        fs::create_dir_all(root.join("sessions")).unwrap();
        let provider = CodexProvider::with_root(root.clone());
        let write = |id: &str, source: Value, extra: Vec<Value>| {
            let mut records = vec![
                json!({"type":"session_meta","payload":{"id":id,"source":source,"cwd":"/tmp"}}),
            ];
            records.extend(extra);
            let path = root.join("sessions").join(format!("{id}.jsonl"));
            fs::write(
                &path,
                records
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .unwrap();
            (path, records)
        };
        let user = |text: &str| json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}});
        let (path, records) = write(
            "main",
            json!("cli"),
            vec![
                user("<recommended_plugins>runtime</recommended_plugins>"),
                json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"Internal planning"}]}}),
                user("Please explain codex-auto-review behavior"),
            ],
        );
        let summary = provider
            .make_session_summary(&path, &HashMap::new())
            .unwrap();
        assert_eq!(
            summary.first_message,
            "Please explain codex-auto-review behavior"
        );
        assert_eq!(summary.kind, SessionKind::Main);
        assert_eq!(
            provider
                .make_session(&path, &records, &HashMap::new(), true)
                .unwrap()
                .0
                .first_message,
            summary.first_message
        );
        let (path, records) = write(
            "child",
            json!({"subagent":{"thread_spawn":{"parent_thread_id":"main","agent_path":"/root/review_ui","agent_nickname":"Newton"}}}),
            vec![user("Inherited parent request")],
        );
        let child = provider
            .make_session_summary(&path, &HashMap::new())
            .unwrap();
        assert_eq!(child.kind, SessionKind::Subagent);
        assert_eq!(child.parent_session_id.as_deref(), Some("main"));
        assert_eq!(child.first_message, "Inherited parent request");
        assert_eq!(
            provider
                .make_session(&path, &records, &HashMap::new(), true)
                .unwrap()
                .0
                .parent_session_id,
            child.parent_session_id
        );
        let (path, records) = write(
            "guardian",
            json!({"subagent":{"other":"guardian"}}),
            vec![user("Control history")],
        );
        assert!(
            provider
                .make_session_summary(&path, &HashMap::new())
                .is_none()
        );
        assert!(
            provider
                .make_session(&path, &records, &HashMap::new(), true)
                .is_none()
        );
        assert_eq!(provider.sessions().unwrap().len(), 2);
        let mut index = HashMap::new();
        index.insert(
            "main".into(),
            IndexEntry {
                id: "main".into(),
                thread_name: Some("Saved title".into()),
                updated_at: None,
            },
        );
        assert_eq!(
            provider
                .make_session_summary(&root.join("sessions/main.jsonl"), &index)
                .unwrap()
                .first_message,
            "Saved title"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn spawned_agent_links_resolve_task_paths_and_ids_within_the_parent() {
        let root = std::env::temp_dir().join(format!("yes-codex-links-{}", std::process::id()));
        fs::create_dir_all(root.join("sessions")).unwrap();
        let meta = |id: &str, parent: &str| json!({"type":"session_meta","payload":{"id":id,"source":{"subagent":{"thread_spawn":{"parent_thread_id":parent,"agent_path":"/root/reference_colors"}}}}});
        fs::write(
            root.join("sessions/child.jsonl"),
            meta("child", "parent").to_string(),
        )
        .unwrap();
        fs::write(
            root.join("sessions/other.jsonl"),
            meta("other", "different-parent").to_string(),
        )
        .unwrap();
        let provider = CodexProvider::with_root(root.clone());
        for output in [
            json!({"task_name":"/root/reference_colors"}),
            json!({"agent_id":"child"}),
        ] {
            let records = vec![
                json!({"type":"response_item","payload":{"type":"function_call","name":"spawn_agent","call_id":"call","arguments":"{\"message\":\"Review colors\"}"}}),
                json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call","output":output.to_string()}}),
            ];
            let mut messages = provider.parse_messages(&records, None);
            provider.link_subagents("parent", &mut messages);
            assert!(
                messages
                    .iter()
                    .all(|message| message.sub_agent_session_id.as_deref() == Some("child"))
            );
            let mut messages = provider.parse_messages(&records, None);
            provider.link_subagents("unrelated", &mut messages);
            assert!(
                messages
                    .iter()
                    .all(|message| message.sub_agent_session_id.is_none())
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn child_detail_excludes_inherited_history_but_keeps_own_delegation() {
        let provider = CodexProvider::with_root(std::env::temp_dir());
        for explicit_ordinals in [false, true] {
            let boundary = if explicit_ordinals { 103 } else { 3 };
            let mut records = vec![
                json!({"type":"session_meta","payload":{"id":"child","subagent_history_start_ordinal":boundary,"source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}}}}),
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":"Parent question"}}),
                json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":"Parent answer"}}),
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":"Child task"}}),
                json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":"Child result"}}),
            ];
            if explicit_ordinals {
                for (index, record) in records.iter_mut().enumerate() {
                    record["ordinal"] = json!(100 + index);
                }
                // Paginated history places the latest metadata before inherited records.
                records[0]["ordinal"] = json!(200);
            }
            let (_, messages) = provider
                .make_session(Path::new("child.jsonl"), &records, &HashMap::new(), true)
                .unwrap();
            assert_eq!(
                messages
                    .iter()
                    .filter_map(|message| message.content.as_deref())
                    .collect::<Vec<_>>(),
                vec!["Child task", "Child result"]
            );
            records[0]["payload"]["source"] = json!("vscode");
            let (_, main_messages) = provider
                .make_session(Path::new("main.jsonl"), &records, &HashMap::new(), true)
                .unwrap();
            assert_eq!(main_messages.len(), 4);
        }
    }

    #[test]
    fn child_titles_prefer_own_tasks_then_use_reply_summaries() {
        let meta = json!({"subagent_history_start_ordinal":1});
        let message = |role: &str, text: &str| json!({"type":"response_item","payload":{"type":"message","role":role,"content":[{"type":"input_text","text":text}]}});
        let inherited = message("user", "Unrelated parent request");
        let task = json!({"type":"response_item","payload":{"type":"agent_message","content":[{"type":"input_text","text":"Message Type: NEW_TASK\nPayload:\nReview sidebar layout"}]}});
        assert_eq!(
            CodexProvider::subagent_content_preview(
                meta.as_object().unwrap(),
                &[inherited.clone(), task]
            ),
            "Review sidebar layout"
        );
        let encrypted = json!({"type":"response_item","payload":{"type":"agent_message","content":[{"type":"input_text","text":"Message Type: NEW_TASK\nPayload:\n"},{"type":"encrypted_content","encrypted_content":"unreadable"}]}});
        assert_eq!(
            CodexProvider::subagent_content_preview(
                meta.as_object().unwrap(),
                &[
                    inherited,
                    encrypted,
                    message("assistant", "Verified sidebar colors and spacing")
                ]
            ),
            "Verified sidebar colors and spacing"
        );
    }

    #[test]
    fn child_title_uses_agent_name_when_task_is_beyond_bounded_prefix() {
        let root =
            std::env::temp_dir().join(format!("yes-codex-title-tail-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("child.jsonl");
        let meta = json!({"type":"session_meta","payload":{"id":"child","timestamp":"2026-09-07T10:00:00Z","subagent_history_start_ordinal":3,"source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent","agent_path":"/root/check_colors"}}}}});
        let inherited = json!({"type":"response_item","timestamp":"2026-09-07T10:00:00Z","payload":{"type":"message","role":"user","content":"x".repeat(300*1024)}});
        let inherited_request = json!({"type":"response_item","timestamp":"2026-09-07T10:00:00Z","payload":{"type":"message","role":"user","content":"Start the parent app"}});
        let reply = json!({"type":"response_item","timestamp":"2026-09-07T10:01:00Z","payload":{"type":"message","role":"assistant","content":"Checked sidebar colors"}});
        fs::write(
            &path,
            format!("{meta}\n{inherited}\n{inherited_request}\n{reply}\n"),
        )
        .unwrap();
        let provider = CodexProvider::with_root(root.clone());
        let summary = provider
            .make_session_summary(&path, &HashMap::new())
            .unwrap();
        assert_eq!(summary.first_message, "check_colors");
        // Without ordinals the tail must not guess from copied timestamps.
        let mut inherited_request = inherited_request;
        inherited_request["ordinal"] = json!(2);
        let mut reply = reply;
        reply["ordinal"] = json!(3);
        fs::write(
            &path,
            format!("{meta}\n{inherited}\n{inherited_request}\n{reply}\n"),
        )
        .unwrap();
        let summary = provider
            .make_session_summary(&path, &HashMap::new())
            .unwrap();
        assert_eq!(summary.first_message, "Checked sidebar colors");
        assert!(summary.agent_type.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hides_codex_runtime_context_messages() {
        assert!(
            CodexProvider::normalize_user("<environment_context>secret</environment_context>")
                .is_empty()
        );
        assert_eq!(CodexProvider::normalize_user("hello"), "hello");
        assert_eq!(
            CodexProvider::normalize_user(
                "<recommended_plugins>list</recommended_plugins>\nReal request"
            ),
            "\nReal request"
        );
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
    #[test]
    fn user_image_only_is_preserved_as_attachment() {
        let provider = CodexProvider::with_root(std::path::PathBuf::from("/tmp/absent"));
        let messages = provider.parse_messages(&[json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,YQ=="}]}})], None);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].attachments.len(), 1);
    }
}
