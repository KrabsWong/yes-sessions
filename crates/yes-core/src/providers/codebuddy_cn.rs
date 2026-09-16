//! CodeBuddy CN stores ordered message references separately from message bodies.
use super::SessionProvider;
use crate::{
    AppType, MessageType, Session, SessionDetail, SessionMessage,
    model::{SessionKind, TokenUsage, ToolOutput},
};
use anyhow::{Context, Result};
use chrono::DateTime;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct CodeBuddyCnProvider {
    root: PathBuf,
    workspace_storage: Option<PathBuf>,
}

fn read(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
fn time(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(Value::as_i64)
        .or_else(|| {
            DateTime::parse_from_rfc3339(&string(value, key))
                .ok()
                .map(|t| t.timestamp_millis())
        })
        .unwrap_or_default()
}
fn decoded(value: &Value) -> Result<Value> {
    match value {
        Value::String(s) => Ok(serde_json::from_str(s)?),
        _ => Ok(value.clone()),
    }
}
fn component(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

// Only unwrap the complete envelope observed in CodeBuddy CN's saved requests.
// A bare user_query tag, arbitrary XML, and incomplete envelopes remain literal.
fn user_envelope(original: &str) -> Option<(&str, &str)> {
    let mut rest = original.trim();
    let mut has_additional_data = false;
    loop {
        let tag = [
            "user_info",
            "rules",
            "git_status",
            "project_context",
            "additional_data",
            "system_reminder",
        ]
        .into_iter()
        .find(|tag| rest.starts_with(&format!("<{tag}>")));
        let Some(tag) = tag else { break };
        let closing = format!("</{tag}>");
        let end = rest.find(&closing)? + closing.len();
        has_additional_data |= tag == "additional_data";
        rest = rest[end..].trim_start();
    }
    if !has_additional_data {
        return None;
    }
    let query = rest
        .strip_prefix("<user_query>")?
        .strip_suffix("</user_query>")?;
    if query.contains("<user_query>") || query.contains("</user_query>") {
        return None;
    }
    let prefix_len = original.trim().len() - rest.len();
    Some((query.trim(), original.trim()[..prefix_len].trim()))
}

fn separate_user_context(message: &mut SessionMessage, extra: &Value) {
    let Some(original) = message.content.as_deref() else {
        return;
    };
    let envelope = user_envelope(original);
    let source = extra["sourceContentBlocks"].as_array().filter(|blocks| {
        !blocks.is_empty()
            && blocks.iter().all(|block| match block["type"].as_str() {
                Some("text") => block["text"].is_string(),
                Some("image") => true,
                _ => false,
            })
    });
    let content = match source {
        Some(blocks) => blocks
            .iter()
            .filter(|block| block["type"] == "text" && block["_meta"]["displayAsContext"] != true)
            .filter_map(|block| block["text"].as_str())
            .collect::<String>(),
        None => match envelope {
            Some((query, _)) => query.to_owned(),
            None => return,
        },
    };
    if content == original {
        return;
    }
    // Retain unrecognized body formatting in the disclosure instead of discarding it.
    let mut context = envelope.map_or(original, |(_, context)| context).to_owned();
    for block in source.into_iter().flatten() {
        if block["_meta"]["displayAsContext"] == true
            && let Some(text) = block["text"].as_str().filter(|text| !text.is_empty())
            && !context.contains(text)
        {
            if !context.is_empty() {
                context.push_str("\n\n");
            }
            context.push_str(text);
        }
    }
    message
        .metadata
        .insert("original_user_content".into(), json!(original));
    if !context.is_empty() {
        message
            .metadata
            .insert("user_context".into(), json!(context));
    }
    message.content = Some(content);
}

impl Default for CodeBuddyCnProvider {
    fn default() -> Self {
        let support = dirs::home_dir()
            .unwrap_or_default()
            .join("Library/Application Support");
        Self {
            root: support.join("CodeBuddyExtension/Data"),
            workspace_storage: Some(support.join("CodeBuddy CN/User/workspaceStorage")),
        }
    }
}
impl CodeBuddyCnProvider {
    pub fn with_root(root: PathBuf) -> Self {
        Self {
            root,
            workspace_storage: None,
        }
    }

    fn workspaces(&self) -> std::collections::HashMap<String, PathBuf> {
        let mut result = std::collections::HashMap::new();
        let Some(root) = &self.workspace_storage else {
            return result;
        };
        let Ok(entries) = fs::read_dir(root) else {
            return result;
        };
        for entry in entries.flatten() {
            let path = entry.path().join("workspace.json");
            let Ok(value) = super::search_path(root, &path).and_then(|path| read(&path)) else {
                continue;
            };
            let Some(attachment) =
                crate::attachments::structured(&json!({"type":"file", "url":value["folder"]}))
            else {
                continue;
            };
            let crate::AttachmentSource::LocalPath(path) = attachment.source else {
                continue;
            };
            // The extension hashes the lexical normalized path, not the symlink target.
            let mut normalized = PathBuf::new();
            for part in path.components() {
                match part {
                    std::path::Component::ParentDir => {
                        normalized.pop();
                    }
                    std::path::Component::CurDir => {}
                    _ => normalized.push(part),
                }
            }
            let hash = format!(
                "{:x}",
                md5::compute(normalized.to_string_lossy().as_bytes())
            );
            result.insert(hash, normalized);
        }
        result
    }

    fn checked(&self, path: &Path) -> Result<PathBuf> {
        super::search_path(&self.root, path)
    }

    fn identity(&self, path: &Path) -> Result<String> {
        let root = self.root.canonicalize()?;
        let path = self.checked(path)?;
        let parts = path
            .strip_prefix(root)?
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        anyhow::ensure!(
            parts.len() == 7
                && parts[1] == "CodeBuddyIDE"
                && parts[3] == "history"
                && parts[6] == "index.json"
                && parts[..6].iter().all(|p| component(p)),
            "Invalid CodeBuddy CN session path"
        );
        Ok(format!(
            "cn:{}:{}:{}:{}",
            parts[0], parts[2], parts[4], parts[5]
        ))
    }

    fn load(&self, session: &Session) -> Result<Option<SessionDetail>> {
        anyhow::ensure!(
            self.identity(&session.file_path)? == session.id,
            "Session path does not match its ID"
        );
        let index = read(&self.checked(&session.file_path)?)?;
        let folder = session
            .file_path
            .parent()
            .context("Missing session directory")?;
        let mut messages = Vec::new();
        let mut offsets = std::collections::HashMap::new();
        for reference in index["messages"].as_array().into_iter().flatten() {
            let id = string(reference, "id");
            anyhow::ensure!(component(&id), "Invalid message ID");
            let path =
                super::search_path(folder, &folder.join("messages").join(format!("{id}.json")))?;
            let record = read(&path).context("Could not read CodeBuddy CN message")?;
            let start = messages.len();
            messages.extend(Self::normalize(&record)?);
            if messages.len() > start {
                offsets.insert(id, start);
            }
        }
        // Usage is a request total, not a per-response count. Attribute it once.
        for request in index["requests"].as_array().into_iter().flatten() {
            let Some(offset) = request["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .find_map(|id| offsets.get(id))
                .copied()
            else {
                continue;
            };
            let usage = &request["usage"];
            let input_tokens = usage["inputTokens"].as_u64();
            let output_tokens = usage["outputTokens"].as_u64();
            let usage = TokenUsage {
                input_tokens,
                output_tokens,
                total_tokens: usage["totalTokens"]
                    .as_u64()
                    .or_else(|| input_tokens?.checked_add(output_tokens?)),
                cache_read_tokens: None,
            };
            if usage != TokenUsage::default() {
                messages[offset].usage = Some(usage);
            }
        }
        let mut session = session.clone();
        session.message_count = messages.len();
        // Keep the indexed title: user bodies can start with injected environment context.
        if let Some(last) = messages
            .iter()
            .rev()
            .find(|m| m.message_type == MessageType::User)
        {
            session.last_message = last
                .content
                .as_deref()
                .unwrap_or_default()
                .chars()
                .take(100)
                .collect();
        }
        Ok(Some(SessionDetail {
            session,
            messages,
            subtree_usage: None,
        }))
    }

    fn normalize(record: &Value) -> Result<Vec<SessionMessage>> {
        let body = decoded(&record["message"])?;
        let extra = decoded(&record["extra"])?;
        let role = body["role"]
            .as_str()
            .or_else(|| record["role"].as_str())
            .unwrap_or_default();
        let kind = match role {
            "user" => MessageType::User,
            "assistant" => MessageType::Assistant,
            "tool" => MessageType::ToolResult,
            _ => MessageType::System,
        };
        let timestamp = string(record, "createdAt");
        let make = |kind, text: String| {
            let mut message = SessionMessage::text(kind, &timestamp, text);
            message.model = extra["modelId"].as_str().map(str::to_owned);
            message
                .metadata
                .insert("codebuddy_cn_message_id".into(), record["id"].clone());
            message
                .metadata
                .insert("codebuddy_cn_extra".into(), extra.clone());
            message.metadata.insert(
                "codebuddy_cn_references".into(),
                record["references"].clone(),
            );
            message.metadata.insert(
                "codebuddy_cn_provider_options".into(),
                body["providerOptions"].clone(),
            );
            message
        };
        let mut messages = Vec::new();
        let mut text = make(kind, String::new());
        let blocks = match &body["content"] {
            Value::Array(blocks) => blocks.clone(),
            Value::String(s) => vec![json!({"type":"text", "text":s})],
            _ => Vec::new(),
        };
        for block in blocks {
            match block["type"].as_str().unwrap_or_default() {
                "text" => text
                    .content
                    .get_or_insert_default()
                    .push_str(&string(&block, "text")),
                "reasoning" => text
                    .reasoning_content
                    .get_or_insert_default()
                    .push_str(&string(&block, "text")),
                "tool-call" | "tool-result" => {
                    if text.content.as_ref().is_some_and(|s| !s.is_empty())
                        || text.reasoning_content.is_some()
                        || !text.attachments.is_empty()
                    {
                        messages.push(text);
                        text = make(kind, String::new());
                    }
                    let call = block["type"] == "tool-call";
                    let mut message = make(
                        if call {
                            MessageType::ToolUse
                        } else {
                            MessageType::ToolResult
                        },
                        String::new(),
                    );
                    message.call_id = block["toolCallId"].as_str().map(str::to_owned);
                    message.tool_name = block["toolName"].as_str().map(str::to_owned);
                    if call {
                        message.tool_input = decoded(&block["args"])?.as_object().cloned();
                    } else {
                        let result = &block["result"];
                        let output = result.as_str().map(str::to_owned).unwrap_or_else(|| {
                            serde_json::to_string_pretty(result).unwrap_or_default()
                        });
                        message.tool_output = Some(ToolOutput {
                            output: Some(output),
                            preview: None,
                            truncated: false,
                            extra: serde_json::Map::from_iter([(
                                "isError".into(),
                                block["isError"].clone(),
                            )]),
                        });
                    }
                    messages.push(message);
                }
                "image" => {
                    let normalized = json!({"type":"image", "url":block["image"]});
                    if let Some(attachment) = crate::attachments::structured(&normalized) {
                        text.attachments.push(attachment);
                    }
                }
                _ => {
                    // Preserve future block types rather than silently losing their content.
                    text.content
                        .get_or_insert_default()
                        .push_str(&serde_json::to_string_pretty(&block)?);
                }
            }
        }
        if text.content.as_ref().is_some_and(|s| !s.is_empty())
            || text.reasoning_content.is_some()
            || !text.attachments.is_empty()
        {
            messages.push(text);
        }
        if kind == MessageType::User {
            // The original body has already supplied attachments; source blocks are
            // used only for text so pasted images are not duplicated.
            for message in &mut messages {
                if message.message_type == MessageType::User {
                    separate_user_context(message, &extra);
                }
            }
        }
        Ok(messages)
    }
}
impl SessionProvider for CodeBuddyCnProvider {
    fn app_type(&self) -> AppType {
        AppType::CodeBuddyCn
    }
    fn is_available(&self) -> bool {
        fs::read_dir(&self.root).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.path().join("CodeBuddyIDE").is_dir())
        })
    }
    fn sessions(&self) -> Result<Vec<Session>> {
        let mut sessions = Vec::new();
        let workspaces = self.workspaces();
        // Depth six reaches workspace indexes, never the per-message directories.
        for entry in WalkDir::new(&self.root)
            .max_depth(6)
            .into_iter()
            .filter_entry(|entry| entry.depth() != 2 || entry.file_name() == "CodeBuddyIDE")
            .filter_map(Result::ok)
        {
            if entry.file_name() != "index.json" || !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let Some(folder) = path.parent() else {
                continue;
            };
            if folder
                .parent()
                .and_then(Path::file_name)
                .is_none_or(|s| s != "history")
            {
                continue;
            }
            let Ok(index) = self.checked(path).and_then(|path| read(&path)) else {
                continue;
            };
            for conversation in index["conversations"].as_array().into_iter().flatten() {
                let uuid = string(conversation, "id");
                if !component(&uuid) {
                    continue;
                }
                let file_path = folder.join(&uuid).join("index.json");
                let Ok(id) = self.identity(&file_path) else {
                    continue;
                };
                let Ok(index) = read(&self.checked(&file_path)?) else {
                    continue;
                };
                let count = index["messages"].as_array().map_or(0, Vec::len);
                if count == 0 {
                    continue;
                }
                let title = string(conversation, "name");
                sessions.push(Session {
                    id,
                    app_type: self.app_type(),
                    file_name: if title.is_empty() {
                        uuid.clone()
                    } else {
                        title.clone()
                    },
                    file_path,
                    created_at: time(conversation, "createdAt"),
                    updated_at: time(conversation, "lastMessageAt"),
                    message_count: count,
                    first_message: title.clone(),
                    last_message: title,
                    directory: folder
                        .file_name()
                        .and_then(|name| name.to_str())
                        .and_then(|hash| workspaces.get(hash))
                        .cloned(),
                    uuid: Some(uuid),
                    kind: SessionKind::Main,
                    parent_session_id: None,
                    agent_type: None,
                });
            }
        }
        sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        Ok(sessions)
    }
    fn session_detail(&self, id: &str) -> Result<Option<SessionDetail>> {
        match self.sessions()?.iter().find(|s| s.id == id) {
            Some(session) => self.load(session),
            None => Ok(None),
        }
    }
    fn session_detail_for_search(&self, session: &Session) -> Result<Option<SessionDetail>> {
        self.load(session)
    }
    fn session_detail_from_summary(&self, session: &Session) -> Result<Option<SessionDetail>> {
        self.load(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_record(original: &str, extra: Value) -> Value {
        json!({"message":{"role":"user","content":[{"type":"text","text":original}]},"extra":extra})
    }

    #[test]
    fn separates_structured_user_input_from_injected_context() {
        let context = "<user_info>\nOS Version: darwin\n</user_info>\n\n<rules>\n<agent_requestable_workspace_rules description=\"rules\">Rule</agent_requestable_workspace_rules>\n</rules>\n\n<git_status>Clean</git_status>\n<additional_data>Editor state</additional_data>";
        let original = format!("{context}\n\n<user_query>\nPlease fix this\n</user_query>");
        let record = user_record(
            &original,
            json!({"sourceContentBlocks":[
                {"type":"text","text":"Please ","_meta":{"displayAsContext":false}},
                {"type":"text","text":"fix this"},
                {"type":"text","text":"Referenced file","_meta":{"displayAsContext":true}}
            ]}),
        );
        let messages = CodeBuddyCnProvider::normalize(&record).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.as_deref(), Some("Please fix this"));
        assert_eq!(messages[0].metadata["original_user_content"], original);
        assert_eq!(
            messages[0].metadata["user_context"],
            format!("{context}\n\nReferenced file")
        );
    }

    #[test]
    fn preserves_user_xml_and_assistant_content() {
        let query =
            "Explain this XML:\n<user_info>mine</user_info>\n<user_query>nested</user_query>";
        let original = format!(
            "<additional_data>Editor</additional_data>\n<user_query>\n{query}\n</user_query>"
        );
        let mut record = user_record(
            &original,
            json!({"sourceContentBlocks":[{"type":"text","text":query}]}),
        );
        let messages = CodeBuddyCnProvider::normalize(&record).unwrap();
        assert_eq!(messages[0].content.as_deref(), Some(query));
        assert_eq!(messages[0].metadata["original_user_content"], original);
        record["message"]["role"] = json!("assistant");
        let messages = CodeBuddyCnProvider::normalize(&record).unwrap();
        assert_eq!(messages[0].content.as_deref(), Some(original.as_str()));
        assert!(!messages[0].metadata.contains_key("user_context"));
    }

    #[test]
    fn keeps_image_only_messages_and_does_not_duplicate_attachments() {
        let original =
            "<additional_data>Editor</additional_data>\n<user_query>\n[Image]\n</user_query>";
        let mut record = user_record(
            original,
            json!({"sourceContentBlocks":[{"type":"image","data":"aGVsbG8=","mimeType":"image/png"}]}),
        );
        record["message"]["content"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"image","image":"data:image/png;base64,aGVsbG8="}));
        let messages = CodeBuddyCnProvider::normalize(&record).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.as_deref(), Some(""));
        assert_eq!(messages[0].attachments.len(), 1);
        assert_eq!(
            messages[0].metadata["user_context"],
            "<additional_data>Editor</additional_data>"
        );
        assert_eq!(messages[0].metadata["original_user_content"], original);
    }

    #[test]
    fn missing_source_only_unwraps_complete_known_envelopes() {
        let context = "<additional_data>Editor</additional_data>\n<system_reminder>Reminder</system_reminder>";
        let original =
            format!("{context}\n<user_query>\nQuestion <example>code</example>\n</user_query>");
        let messages = CodeBuddyCnProvider::normalize(&user_record(&original, json!({}))).unwrap();
        assert_eq!(
            messages[0].content.as_deref(),
            Some("Question <example>code</example>")
        );
        assert_eq!(messages[0].metadata["user_context"], context);
        for literal in [
            "<user_query>literal XML</user_query>",
            "<user_info>literal XML</user_info>",
            "<additional_data>Editor</additional_data><user_query>unfinished",
            "<additional_data>Editor</additional_data><unknown>data</unknown><user_query>Question</user_query>",
            "<additional_data>Editor</additional_data><user_query>Question</user_query>more text",
            "<additional_data>Editor</additional_data><user_query><user_query>nested</user_query></user_query>",
            "Regular question",
        ] {
            let messages =
                CodeBuddyCnProvider::normalize(&user_record(literal, json!({}))).unwrap();
            assert_eq!(messages[0].content.as_deref(), Some(literal));
            assert!(!messages[0].metadata.contains_key("user_context"));
        }
    }

    #[test]
    fn unchanged_or_unrecognized_source_blocks_preserve_original() {
        for extra in [
            json!({"sourceContentBlocks":[]}),
            json!({"sourceContentBlocks":[{"type":"future","value":"data"}]}),
            json!({"sourceContentBlocks":[{"type":"text","text":"Question"}]}),
            json!({"sourceContentBlocks":[{"type":"text","text":null}]}),
        ] {
            let messages = CodeBuddyCnProvider::normalize(&user_record("Question", extra)).unwrap();
            assert_eq!(messages[0].content.as_deref(), Some("Question"));
            assert!(!messages[0].metadata.contains_key("user_context"));
            assert!(!messages[0].metadata.contains_key("original_user_content"));
        }
    }

    #[test]
    fn decodes_nested_messages_tools_reasoning_images_and_context() {
        let record = json!({"id":"message", "createdAt":"2026-09-16T00:00:00Z",
            "references":[{"type":"rules", "content":"rule"}],
            "extra":json!({"modelId":"test-model"}).to_string(),
            "message":json!({"role":"assistant", "providerOptions":{"key":"value"}, "content":[
                {"type":"reasoning","text":"Thinking"}, {"type":"text","text":"Answer"},
                {"type":"tool-call","toolCallId":"call","toolName":"read_file","args":{"path":"a.rs"}},
                {"type":"tool-result","toolCallId":"call","toolName":"read_file","result":{"success":true,"result":{"content":"file body"}},"isError":false},
                {"type":"image","image":"data:image/png;base64,aGVsbG8="}
            ]}).to_string()});
        let messages = CodeBuddyCnProvider::normalize(&record).unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].content.as_deref(), Some("Answer"));
        assert_eq!(messages[0].reasoning_content.as_deref(), Some("Thinking"));
        assert_eq!(messages[0].model.as_deref(), Some("test-model"));
        assert_eq!(
            messages[0].metadata["codebuddy_cn_references"][0]["content"],
            "rule"
        );
        assert_eq!(
            messages[0].metadata["codebuddy_cn_provider_options"]["key"],
            "value"
        );
        assert_eq!(messages[1].tool_input.as_ref().unwrap()["path"], "a.rs");
        assert_eq!(messages[1].call_id, messages[2].call_id);
        assert!(
            messages[2]
                .tool_output
                .as_ref()
                .unwrap()
                .output
                .as_ref()
                .unwrap()
                .contains("file body")
        );
        assert_eq!(messages[3].attachments.len(), 1);
    }

    #[test]
    fn indexes_are_lightweight_details_ordered_and_paths_confined() {
        let root = std::env::temp_dir().join(format!("yes-cn-test-{}", std::process::id()));
        let workspace = root.join("account/CodeBuddyIDE/account/history/workspace");
        let session = workspace.join("session");
        fs::create_dir_all(session.join("messages")).unwrap();
        fs::write(workspace.join("index.json"),json!({"conversations":[{"id":"session","name":"A title","createdAt":"2026-09-16T00:00:00Z","lastMessageAt":"2026-09-16T00:01:00Z"},{"id":"../../escape"}]}).to_string()).unwrap();
        let index = json!({"messages":[{"id":"user"},{"id":"reply"}],"requests":[{"id":"request","messages":["user","reply"],"usage":{"inputTokens":100,"outputTokens":20,"totalTokens":120}}]});
        fs::write(session.join("index.json"), index.to_string()).unwrap();
        let provider = CodeBuddyCnProvider::with_root(root.clone());
        // Listing works even before bodies exist; opening reports missing data.
        let sessions = provider.sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].message_count, 2);
        assert!(provider.load(&sessions[0]).is_err());
        for (id, role, text) in [
            ("user", "user", "Question"),
            ("reply", "assistant", "Answer"),
        ] {
            fs::write(session.join(format!("messages/{id}.json")),json!({"id":id,"createdAt":"2026-09-16T00:00:00Z","message":json!({"role":role,"content":[{"type":"text","text":text}]}).to_string(),"extra":"{}"}).to_string()).unwrap();
        }
        let detail = provider.session_detail(&sessions[0].id).unwrap().unwrap();
        assert_eq!(detail.messages[0].content.as_deref(), Some("Question"));
        assert_eq!(detail.session.first_message, "A title");
        assert_eq!(detail.session.file_name, sessions[0].file_name);
        assert_eq!(detail.messages[1].content.as_deref(), Some("Answer"));
        assert_eq!(
            detail.messages[0].usage.as_ref().unwrap().total_tokens,
            Some(120)
        );
        assert!(detail.messages[1].usage.is_none());
        assert_eq!(
            detail,
            provider
                .session_detail_for_search(&sessions[0])
                .unwrap()
                .unwrap()
        );
        let mut wrong_id = sessions[0].clone();
        wrong_id.id.push('x');
        assert!(provider.load(&wrong_id).is_err());
        let outside = root.with_extension("json");
        fs::write(&outside, "{}").unwrap();
        fs::remove_file(session.join("messages/user.json")).unwrap();
        std::os::unix::fs::symlink(&outside, session.join("messages/user.json")).unwrap();
        assert!(provider.load(&sessions[0]).is_err());
        let mut invalid = index;
        invalid["messages"][0]["id"] = json!("../../outside");
        fs::write(session.join("index.json"), invalid.to_string()).unwrap();
        assert!(provider.load(&sessions[0]).is_err());
        fs::remove_file(outside).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_uri_maps_to_extension_hash() {
        let root = std::env::temp_dir().join(format!("yes-cn-workspace-{}", std::process::id()));
        fs::create_dir_all(root.join("entry")).unwrap();
        fs::write(
            root.join("entry/workspace.json"),
            r#"{"folder":"file:///tmp/project%20name"}"#,
        )
        .unwrap();
        let provider = CodeBuddyCnProvider {
            root: root.clone(),
            workspace_storage: Some(root.clone()),
        };
        let expected = format!("{:x}", md5::compute(b"/tmp/project name"));
        assert_eq!(
            provider.workspaces()[&expected],
            PathBuf::from("/tmp/project name")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
