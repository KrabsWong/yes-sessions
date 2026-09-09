use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{TimeZone as _, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, params};
use serde_json::{Map, Value, json};

use crate::{
    AppType, MessageType, Session, SessionDetail, SessionMessage,
    model::{SessionKind, ToolOutput},
};

use super::SessionProvider;

#[derive(Debug, Clone)]
pub struct OpenCodeProvider {
    database_path: PathBuf,
}

#[derive(Debug)]
struct SessionRow {
    id: String,
    directory: String,
    title: String,
    created_at: i64,
    updated_at: i64,
    message_count: usize,
    parent_id: Option<String>,
}

impl Default for OpenCodeProvider {
    fn default() -> Self {
        Self {
            database_path: dirs::home_dir()
                .unwrap_or_default()
                .join(".local/share/opencode/opencode.db"),
        }
    }
}

impl OpenCodeProvider {
    pub fn with_database_path(database_path: PathBuf) -> Self {
        Self { database_path }
    }

    fn connection(&self) -> Result<Connection> {
        Connection::open_with_flags(&self.database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("open OpenCode database {}", self.database_path.display()))
    }

    fn tables(connection: &Connection) -> Result<Vec<(&'static str, &'static str)>> {
        let mut tables = Vec::new();
        for (sessions, messages) in [("session_v2", "session_message"), ("session", "message")] {
            if connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [sessions],
                |row| row.get::<_, bool>(0),
            )? {
                tables.push((sessions, messages));
            }
        }
        Ok(tables)
    }

    pub(crate) fn is_v2_session(&self, session_id: &str) -> Result<bool> {
        let connection = self.connection()?;
        if !Self::tables(&connection)?
            .iter()
            .any(|(table, _)| *table == "session_v2")
        {
            return Ok(false);
        }
        Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_v2 WHERE id = ?1)",
            [session_id],
            |row| row.get(0),
        )?)
    }

    fn iso_time(timestamp: i64) -> String {
        Utc.timestamp_millis_opt(timestamp)
            .single()
            .unwrap_or_else(Utc::now)
            .to_rfc3339()
    }

    fn model_name(value: Option<&Value>) -> Option<String> {
        match value {
            Some(Value::String(model)) => Some(model.clone()),
            Some(Value::Object(model)) => model
                .get("modelID")
                .or_else(|| model.get("model"))
                .or_else(|| model.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            _ => None,
        }
    }

    fn parse_message(
        data: &Value,
        parts: &[Value],
        timestamp: i64,
        inherited_model: Option<String>,
    ) -> Vec<SessionMessage> {
        let role = data
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("assistant");
        let model =
            Self::model_name(data.get("modelID").or_else(|| data.get("model"))).or(inherited_model);
        let mut content = Vec::new();
        let mut reasoning = Vec::new();
        let mut tool_parts = Vec::new();

        for part in parts {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        content.push(text.to_owned())
                    }
                }
                Some("reasoning") => {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        reasoning.push(text.to_owned())
                    }
                }
                Some("tool") => tool_parts.push(part),
                _ => {}
            }
        }

        if role == "assistant" && !tool_parts.is_empty() {
            return tool_parts
                .into_iter()
                .enumerate()
                .map(|(index, tool)| {
                    let state = tool.get("state").and_then(Value::as_object);
                    let mut message = SessionMessage::text(
                        MessageType::ToolUse,
                        Self::iso_time(timestamp),
                        if index == 0 {
                            content.join("\n\n")
                        } else {
                            String::new()
                        },
                    );
                    message.tool_name = Some(
                        tool.get("tool")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_owned(),
                    );
                    message.tool_input = state
                        .and_then(|value| value.get("input"))
                        .and_then(Value::as_object)
                        .cloned()
                        .or_else(|| Some(Map::new()));
                    message.tool_output = state
                        .and_then(|value| value.get("output").or_else(|| value.get("error")))
                        .map(|output| ToolOutput {
                            output: Some(if let Some(text) = output.as_str() {
                                text.to_owned()
                            } else {
                                serde_json::to_string_pretty(output).unwrap_or_default()
                            }),
                            preview: None,
                            truncated: false,
                            extra: Map::new(),
                        });
                    message.call_id = tool
                        .get("callID")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    message.reasoning_content =
                        (index == 0 && !reasoning.is_empty()).then(|| reasoning.join("\n\n"));
                    if let Some(state) = state {
                        if let Some(status) = state.get("status") {
                            message.metadata.insert("subtype".into(), status.clone());
                        }
                        if message.tool_name.as_deref() == Some("task") {
                            message.sub_agent_session_id = state
                                .get("metadata")
                                .and_then(|metadata| metadata.get("sessionId"))
                                .and_then(Value::as_str)
                                .map(str::to_owned);
                        }
                    }
                    message.model = model.clone();
                    if index == 0 {
                        crate::attachments::normalize(
                            &mut message,
                            Some(&Value::Array(parts.to_vec())),
                        );
                    }
                    message
                })
                .collect();
        }

        let mut message = SessionMessage::text(
            if role == "user" {
                MessageType::User
            } else {
                MessageType::Assistant
            },
            Self::iso_time(timestamp),
            content.join("\n\n"),
        );
        message.reasoning_content = (!reasoning.is_empty()).then(|| reasoning.join("\n\n"));
        message.model = model;
        crate::attachments::normalize(&mut message, Some(&Value::Array(parts.to_vec())));
        vec![message]
    }

    fn base_session(&self, row: SessionRow) -> Session {
        let kind = if row.parent_id.is_some() {
            SessionKind::Subagent
        } else {
            SessionKind::Main
        };
        // OpenCode appends the role to generated child titles; show it as a badge.
        let (title, agent_type) = if kind == SessionKind::Subagent {
            row.title
                .strip_suffix(" subagent)")
                .and_then(|title| title.rsplit_once(" (@"))
                .map(|(title, role)| (title.to_owned(), Some(role.to_owned())))
                .unwrap_or_else(|| (row.title.clone(), None))
        } else {
            (row.title.clone(), None)
        };
        Session {
            id: row.id,
            app_type: AppType::OpenCode,
            file_name: title.clone(),
            file_path: self.database_path.clone(),
            created_at: row.created_at,
            updated_at: row.updated_at,
            message_count: row.message_count,
            first_message: title,
            last_message: String::new(),
            directory: Some(PathBuf::from(row.directory)),
            uuid: None,
            kind,
            parent_session_id: row.parent_id,
            agent_type,
        }
    }
}

impl SessionProvider for OpenCodeProvider {
    fn app_type(&self) -> AppType {
        AppType::OpenCode
    }
    fn is_available(&self) -> bool {
        self.database_path.exists()
    }

    fn sessions(&self) -> Result<Vec<Session>> {
        let connection = self.connection()?;
        let tables = Self::tables(&connection)?;
        let has_v2 = tables.iter().any(|(table, _)| *table == "session_v2");
        let mut sessions = Vec::new();
        for (table, messages) in tables {
            // A migrated row is authoritative even when archived in v2.
            let exclude_migrated = if table == "session" && has_v2 {
                "AND NOT EXISTS (SELECT 1 FROM session_v2 v WHERE v.id = s.id)"
            } else {
                ""
            };
            let mut statement = connection.prepare(&format!(
                "SELECT s.id, s.directory, COALESCE(s.title, ''), s.time_created, s.time_updated, \
                 (SELECT COUNT(*) FROM {messages} m WHERE m.session_id = s.id), s.parent_id \
                 FROM {table} s WHERE s.time_archived IS NULL {exclude_migrated}"
            ))?;
            let rows = statement.query_map([], |row| {
                Ok(SessionRow {
                    id: row.get(0)?,
                    directory: row.get(1)?,
                    title: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    message_count: row.get(5)?,
                    parent_id: row.get(6)?,
                })
            })?;
            for row in rows {
                sessions.push(self.base_session(row?));
            }
        }
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        Ok(sessions)
    }

    fn session_detail(&self, session_id: &str) -> Result<Option<SessionDetail>> {
        let connection = self.connection()?;
        for (table, _) in Self::tables(&connection)? {
            let row = connection.query_row(
                &format!("SELECT id, directory, COALESCE(title, ''), time_created, time_updated, parent_id \
                 FROM {table} WHERE id = ?1"),
                params![session_id],
                |row| Ok(SessionRow {
                    id: row.get(0)?, directory: row.get(1)?, title: row.get(2)?,
                    created_at: row.get(3)?, updated_at: row.get(4)?,
                    message_count: 0, parent_id: row.get(5)?,
                }),
            ).optional()?;
            let Some(row) = row else { continue };
            let mut messages = if table == "session_v2" {
                Self::v2_messages(&connection, session_id)?
            } else {
                Self::legacy_messages(&connection, session_id)?
            };
            let mut children =
                connection.prepare(&format!("SELECT id FROM {table} WHERE parent_id = ?1"))?;
            let child_ids = children
                .query_map(params![session_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
            for message in &mut messages {
                if message
                    .sub_agent_session_id
                    .as_ref()
                    .is_some_and(|id| !child_ids.contains(id))
                {
                    message.sub_agent_session_id = None;
                }
            }
            let mut session = self.base_session(row);
            session.message_count = messages.len();
            session.last_message = messages
                .last()
                .and_then(|message| message.content.clone())
                .unwrap_or_default();
            return Ok(Some(SessionDetail { session, messages }));
        }
        Ok(None)
    }
}

impl OpenCodeProvider {
    fn legacy_messages(connection: &Connection, session_id: &str) -> Result<Vec<SessionMessage>> {
        let mut parts_statement = connection.prepare(
            "SELECT data FROM part WHERE session_id = ?1 AND message_id = ?2 ORDER BY time_created ASC",
        )?;
        let mut message_statement = connection.prepare(
            "SELECT id, time_created, data FROM message WHERE session_id = ?1 ORDER BY time_created ASC",
        )?;
        let message_rows = message_statement.query_map(params![session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        let mut current_model = None;
        let mut messages = Vec::new();
        for result in message_rows {
            let (message_id, timestamp, data_source) = result?;
            let data: Value = serde_json::from_str(&data_source).unwrap_or(Value::Null);
            current_model = Self::model_name(data.get("modelID").or_else(|| data.get("model")))
                .or(current_model);
            let part_rows = parts_statement.query_map(params![session_id, message_id], |row| {
                row.get::<_, String>(0)
            })?;
            let parts = part_rows
                .filter_map(Result::ok)
                .filter_map(|source| serde_json::from_str(&source).ok())
                .collect::<Vec<Value>>();
            messages.extend(Self::parse_message(
                &data,
                &parts,
                timestamp,
                current_model.clone(),
            ));
        }

        Ok(messages)
    }

    fn v2_messages(connection: &Connection, session_id: &str) -> Result<Vec<SessionMessage>> {
        let mut statement = connection.prepare(
            "SELECT type, time_created, data FROM session_message WHERE session_id = ?1 ORDER BY seq ASC"
        )?;
        let rows = statement.query_map([session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut messages = Vec::new();
        let mut model = None;
        for row in rows {
            let (kind, timestamp, source) = row?;
            let data: Value = serde_json::from_str(&source)?;
            model = Self::model_name(data.get("model")).or(model);
            messages.extend(Self::parse_v2_message(
                &kind,
                &data,
                timestamp,
                model.clone(),
            ));
        }
        Ok(messages)
    }

    fn parse_v2_message(
        kind: &str,
        data: &Value,
        timestamp: i64,
        model: Option<String>,
    ) -> Vec<SessionMessage> {
        if matches!(kind, "synthetic" | "compaction") {
            let mut message = SessionMessage::text(
                MessageType::System,
                Self::iso_time(timestamp),
                data.get(if kind == "compaction" {
                    "summary"
                } else {
                    "text"
                })
                .and_then(Value::as_str)
                .unwrap_or_default(),
            );
            message.metadata.insert("subtype".into(), json!(kind));
            return vec![message];
        }
        if !matches!(kind, "user" | "assistant") {
            return Vec::new();
        }
        let mut parts = if kind == "user" {
            vec![
                json!({"type":"text", "text":data.get("text").and_then(Value::as_str).unwrap_or_default()}),
            ]
        } else {
            data.get("content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        let mut tool_files = Vec::new();
        for part in &mut parts {
            if part.get("type").and_then(Value::as_str) == Some("tool") {
                part["tool"] = part.get("name").cloned().unwrap_or(Value::Null);
                part["callID"] = part.get("id").cloned().unwrap_or(Value::Null);
                let mut files = Vec::new();
                if let Some(state) = part.get_mut("state").and_then(Value::as_object_mut) {
                    if let Some(content) = state.get("content").and_then(Value::as_array) {
                        for file in content
                            .iter()
                            .filter(|part| part.get("type").and_then(Value::as_str) == Some("file"))
                        {
                            files.push(json!({"type":"file", "mime":file.get("mime"), "url":file.get("uri")}));
                        }
                        let output = content
                            .iter()
                            .filter_map(|part| part.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        if !output.is_empty() {
                            state.insert("output".into(), json!(output));
                        }
                    }
                }
                tool_files.push(Value::Array(files));
            }
        }
        if let Some(files) = data.get("files").and_then(Value::as_array) {
            for file in files {
                let mime = file
                    .get("mime")
                    .and_then(Value::as_str)
                    .unwrap_or("application/octet-stream");
                // V2 inline files store raw base64, not a URL.
                let url = file
                    .get("data")
                    .and_then(Value::as_str)
                    .map(|data| format!("data:{mime};base64,{data}"));
                parts.push(
                    json!({"type":"file", "filename":file.get("name"), "mime":mime, "url":url}),
                );
            }
        }
        let mut messages = Self::parse_message(&json!({"role":kind}), &parts, timestamp, model);
        for (message, files) in messages.iter_mut().zip(&tool_files) {
            crate::attachments::normalize(message, Some(files));
        }
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn discovers_v2_sessions_without_duplicates_and_reads_messages_in_sequence() {
        let path = std::env::temp_dir().join(format!("yes-opencode-v2-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(r#"
            CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, title TEXT, time_created INTEGER, time_updated INTEGER, time_archived INTEGER, parent_id TEXT);
            CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
            CREATE TABLE part (session_id TEXT, message_id TEXT, time_created INTEGER, data TEXT);
            CREATE TABLE session_v2 (id TEXT PRIMARY KEY, directory TEXT, title TEXT, time_created INTEGER, time_updated INTEGER, time_archived INTEGER, parent_id TEXT);
            CREATE TABLE session_message (id TEXT, session_id TEXT, type TEXT, seq INTEGER, time_created INTEGER, data TEXT);
            INSERT INTO session VALUES ('migrated','/tmp','Old title',1,2,NULL,NULL),
                ('legacy','/tmp','Legacy only',1,2,NULL,NULL), ('archived','/tmp','Old active row',1,2,NULL,NULL);
            INSERT INTO message VALUES ('legacy-message','legacy',1,'{"role":"user"}');
            INSERT INTO part VALUES ('legacy','legacy-message',1,'{"type":"text","text":"Legacy text"}');
            INSERT INTO session_v2 VALUES ('migrated','/tmp','New title',1,3,NULL,NULL),
                ('new','/tmp',NULL,4,5,NULL,NULL), ('child','/tmp','Inspect (@explore subagent)',4,5,NULL,'new'),
                ('archived','/tmp','Archived in v2',1,3,4,NULL);
            INSERT INTO session_message VALUES ('migrated-message','migrated','user',1,1,'{"text":"Migrated text"}'),
                ('assistant','new','assistant',2,10,'{"model":{"id":"v2-model"},"content":[{"type":"reasoning","text":"Thinking"},{"type":"tool","id":"call-1","name":"task","state":{"status":"completed","input":{"description":"Inspect"},"content":[{"type":"text","text":"Tool result"}],"metadata":{"sessionId":"child"}}}]}'),
                ('user','new','user',1,20,'{"text":"New prompt"}'),
                ('system','new','synthetic',3,30,'{"text":"System context"}');
        "#).unwrap();
        let provider = OpenCodeProvider::with_database_path(path.clone());
        let sessions = provider.sessions().unwrap();
        assert_eq!(sessions.len(), 4);
        assert!(
            sessions
                .windows(2)
                .all(|pair| pair[0].updated_at >= pair[1].updated_at)
        );
        assert!(!sessions.iter().any(|session| session.id == "archived"));
        assert_eq!(
            sessions
                .iter()
                .find(|session| session.id == "migrated")
                .unwrap()
                .first_message,
            "New title"
        );
        assert_eq!(
            sessions
                .iter()
                .find(|session| session.id == "new")
                .unwrap()
                .message_count,
            3
        );
        assert!(provider.is_v2_session("migrated").unwrap());
        assert!(!provider.is_v2_session("legacy").unwrap());
        assert!(!provider.is_v2_session("missing").unwrap());
        let detail = provider.session_detail("new").unwrap().unwrap();
        assert_eq!(detail.messages.len(), 3);
        assert_eq!(detail.messages[0].message_type, MessageType::User);
        assert_eq!(detail.messages[0].content.as_deref(), Some("New prompt"));
        let tool = &detail.messages[1];
        assert_eq!(tool.model.as_deref(), Some("v2-model"));
        assert_eq!(tool.reasoning_content.as_deref(), Some("Thinking"));
        assert_eq!(tool.tool_name.as_deref(), Some("task"));
        assert_eq!(tool.call_id.as_deref(), Some("call-1"));
        assert_eq!(tool.tool_input.as_ref().unwrap()["description"], "Inspect");
        assert_eq!(
            tool.tool_output.as_ref().unwrap().output.as_deref(),
            Some("Tool result")
        );
        assert_eq!(tool.sub_agent_session_id.as_deref(), Some("child"));
        assert_eq!(detail.messages[2].message_type, MessageType::System);
        assert_eq!(
            provider
                .session_detail("migrated")
                .unwrap()
                .unwrap()
                .messages[0]
                .content
                .as_deref(),
            Some("Migrated text")
        );
        assert_eq!(
            provider.session_detail("legacy").unwrap().unwrap().messages[0]
                .content
                .as_deref(),
            Some("Legacy text")
        );
        assert!(provider.session_detail("missing").unwrap().is_none());
        let stats = provider.stats().unwrap();
        assert_eq!(stats.total_sessions, 4);
        assert_eq!(stats.total_messages, 5);
        assert_eq!(stats.first_session_date, Some(1));
        assert_eq!(stats.last_session_date, Some(5));
        connection
            .execute_batch("DROP TABLE session; DROP TABLE message; DROP TABLE part;")
            .unwrap();
        assert_eq!(provider.sessions().unwrap().len(), 3);
        assert_eq!(provider.stats().unwrap().total_messages, 4);
        assert!(provider.session_detail("new").unwrap().is_some());
        drop(connection);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn v2_preserves_attachments_errors_and_system_messages() {
        let user = OpenCodeProvider::parse_v2_message(
            "user",
            &json!({
                "text":"See image", "files":[{"name":"image.png", "mime":"image/png", "data":"aGVsbG8=", "source":{"type":"inline"}}]
            }),
            1,
            None,
        );
        assert_eq!(user[0].attachments.len(), 1);
        assert_eq!(user[0].attachments[0].name, "image.png");
        assert_eq!(
            user[0].attachments[0].source,
            crate::AttachmentSource::DataUrl("data:image/png;base64,aGVsbG8=".into())
        );
        let messages = OpenCodeProvider::parse_v2_message(
            "assistant",
            &json!({"content":[
                {"type":"text", "text":"Checking"},
                {"type":"tool", "id":"one", "name":"read", "state":{"status":"completed", "input":{}, "content":[{"type":"text", "text":"Output"}]}},
                {"type":"tool", "id":"two", "name":"read", "state":{"status":"error", "input":{}, "content":[{"type":"file", "mime":"image/png", "uri":"data:image/png;base64,aGVsbG8="}], "error":{"type":"Error", "message":"Cannot read"}}}
            ]}),
            2,
            Some("model".into()),
        );
        assert_eq!(messages.len(), 2);
        assert!(messages[0].attachments.is_empty());
        assert_eq!(messages[1].attachments.len(), 1);
        assert_eq!(messages[0].content.as_deref(), Some("Checking"));
        assert_eq!(messages[1].content.as_deref(), Some(""));
        assert!(
            messages[1]
                .tool_output
                .as_ref()
                .unwrap()
                .output
                .as_ref()
                .unwrap()
                .contains("Cannot read")
        );
        let summary = OpenCodeProvider::parse_v2_message(
            "compaction",
            &json!({"summary":"Earlier work", "status":"completed"}),
            3,
            None,
        );
        assert_eq!(summary[0].message_type, MessageType::System);
        assert_eq!(summary[0].content.as_deref(), Some("Earlier work"));
        assert!(OpenCodeProvider::parse_v2_message("future-event", &json!({}), 4, None).is_empty());
    }

    #[test]
    fn groups_children_and_links_only_existing_child_sessions() {
        let path = std::env::temp_dir().join(format!("yes-opencode-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(r#"CREATE TABLE session (id TEXT, directory TEXT, title TEXT, time_created INTEGER, time_updated INTEGER, time_archived INTEGER, parent_id TEXT);
            CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
            CREATE TABLE part (session_id TEXT, message_id TEXT, time_created INTEGER, data TEXT);
            INSERT INTO session VALUES ('parent','/tmp','Project overview',1,2,NULL,NULL),
                ('child','/tmp','Inspect rendering (@explore subagent)',1,2,NULL,'parent'),
                ('other','/tmp','Another session',1,2,NULL,NULL);
            INSERT INTO message VALUES ('message','parent',1,'{"role":"assistant","modelID":"actual-model"}');"#).unwrap();
        for (index, target) in ["child", "missing", "other"].iter().enumerate() {
            let part = json!({"type":"tool","tool":"task","callID":target,"state":{
                "status":"completed","input":{"description":"Inspect rendering"},
                "output":"Done","metadata":{"sessionId":target}
            }});
            connection
                .execute(
                    "INSERT INTO part VALUES ('parent','message',?1,?2)",
                    params![index, part.to_string()],
                )
                .unwrap();
        }
        drop(connection);
        let provider = OpenCodeProvider::with_database_path(path.clone());
        assert!(!provider.is_v2_session("parent").unwrap());
        let sessions = provider.sessions().unwrap();
        let child = sessions
            .iter()
            .find(|session| session.id == "child")
            .unwrap();
        assert_eq!(child.kind, SessionKind::Subagent);
        assert_eq!(child.parent_session_id.as_deref(), Some("parent"));
        assert_eq!(child.first_message, "Inspect rendering");
        assert_eq!(child.agent_type.as_deref(), Some("explore"));
        let detail = provider.session_detail("parent").unwrap().unwrap();
        assert_eq!(detail.messages.len(), 3);
        assert_eq!(
            detail.messages[0].sub_agent_session_id.as_deref(),
            Some("child")
        );
        assert!(
            detail.messages[1..]
                .iter()
                .all(|message| message.sub_agent_session_id.is_none())
        );
        assert!(
            detail
                .messages
                .iter()
                .all(|message| message.model.as_deref() == Some("actual-model"))
        );
        assert_eq!(detail.messages[0].metadata["subtype"], "completed");
        let detail = provider.session_detail("child").unwrap().unwrap();
        assert_eq!(detail.session.parent_session_id, child.parent_session_id);
        assert_eq!(detail.session.first_message, child.first_message);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn preserves_every_tool_call_and_output_without_duplicating_prose() {
        let messages = OpenCodeProvider::parse_message(
            &json!({"role": "assistant"}),
            &[
                json!({"type": "text", "text": "Checking files"}),
                json!({"type": "reasoning", "text": "Need both files"}),
                json!({"type": "tool", "tool": "read", "callID": "one", "state": {"input": {"path": "a"}, "output": "first result"}}),
                json!({"type": "tool", "tool": "read", "callID": "two", "state": {"input": {"path": "b"}, "output": {"ok": true}}}),
            ],
            1,
            Some("model".into()),
        );
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].call_id.as_deref(), Some("one"));
        assert_eq!(messages[1].call_id.as_deref(), Some("two"));
        assert_eq!(messages[1].tool_input.as_ref().unwrap()["path"], "b");
        assert_eq!(
            messages[0].tool_output.as_ref().unwrap().output.as_deref(),
            Some("first result")
        );
        assert_eq!(
            serde_json::from_str::<Value>(
                messages[1]
                    .tool_output
                    .as_ref()
                    .unwrap()
                    .output
                    .as_ref()
                    .unwrap()
            )
            .unwrap(),
            json!({"ok": true})
        );
        assert_eq!(messages[0].content.as_deref(), Some("Checking files"));
        assert_eq!(
            messages[0].reasoning_content.as_deref(),
            Some("Need both files")
        );
        assert!(
            messages[1]
                .content
                .as_deref()
                .unwrap_or_default()
                .is_empty()
        );
        assert!(messages[1].reasoning_content.is_none());
        assert!(
            messages
                .iter()
                .all(|message| message.model.as_deref() == Some("model"))
        );
        let user = OpenCodeProvider::parse_message(
            &json!({"role": "user"}),
            &[json!({"type": "text", "text": "hello"})],
            1,
            None,
        );
        assert_eq!(user.len(), 1);
        assert_eq!(user[0].message_type, MessageType::User);
        assert_eq!(user[0].content.as_deref(), Some("hello"));
    }

    #[test]
    fn accepts_both_opencode_model_shapes() {
        assert_eq!(
            OpenCodeProvider::model_name(Some(&json!("claude-4"))).as_deref(),
            Some("claude-4")
        );
        assert_eq!(
            OpenCodeProvider::model_name(Some(&json!({"providerID":"x", "modelID":"gpt-5"})))
                .as_deref(),
            Some("gpt-5")
        );
    }
    #[test]
    fn file_parts_preserve_images_and_other_resources() {
        let messages = OpenCodeProvider::parse_message(
            &json!({"role":"user"}),
            &[
                json!({"type":"file","mime":"image/png","filename":"image.png","url":"file:///tmp/image.png"}),
                json!({"type":"file","mime":"application/pdf","filename":"doc.pdf","url":"file:///tmp/doc.pdf"}),
            ],
            0,
            None,
        );
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].attachments.len(), 2);
    }
    #[test]
    fn tool_messages_keep_file_parts_even_with_source_metadata() {
        let messages = OpenCodeProvider::parse_message(
            &json!({"role":"assistant"}),
            &[
                json!({"type":"tool","tool":"read","state":{"status":"completed"}}),
                json!({"type":"file","mime":"application/pdf","filename":"doc.pdf","url":"file:///tmp/doc.pdf","source":{"type":"file","path":"doc.pdf"}}),
            ],
            0,
            None,
        );
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].attachments.len(), 1);
    }
}
