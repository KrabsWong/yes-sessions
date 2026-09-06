use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{TimeZone as _, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, params};
use serde_json::{Map, Value};

use crate::{
    AppType, MessageType, Session, SessionDetail, SessionMessage, SessionStats,
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
        let model = Self::model_name(data.get("model")).or(inherited_model);
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
                    message.tool_output =
                        state
                            .and_then(|value| value.get("output"))
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
                    message.model = model.clone();
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
        vec![message]
    }

    fn base_session(&self, row: SessionRow) -> Session {
        Session {
            id: row.id,
            app_type: AppType::OpenCode,
            file_name: row.title.clone(),
            file_path: self.database_path.clone(),
            created_at: row.created_at,
            updated_at: row.updated_at,
            message_count: row.message_count,
            first_message: row.title,
            last_message: String::new(),
            directory: Some(PathBuf::from(row.directory)),
            uuid: None,
            kind: SessionKind::Main,
            parent_session_id: None,
            agent_type: None,
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
        let mut statement = connection.prepare(
            "SELECT s.id, s.directory, COALESCE(s.title, ''), s.time_created, s.time_updated, COUNT(m.id) \
             FROM session s LEFT JOIN message m ON m.session_id = s.id \
             WHERE s.time_archived IS NULL GROUP BY s.id ORDER BY s.time_updated DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                directory: row.get(1)?,
                title: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                message_count: row.get(5)?,
            })
        })?;
        Ok(rows
            .filter_map(Result::ok)
            .map(|row| self.base_session(row))
            .collect())
    }

    fn session_detail(&self, session_id: &str) -> Result<Option<SessionDetail>> {
        let connection = self.connection()?;
        let row = connection
            .query_row(
                "SELECT id, directory, COALESCE(title, ''), time_created, time_updated \
             FROM session WHERE id = ?1",
                params![session_id],
                |row| {
                    Ok(SessionRow {
                        id: row.get(0)?,
                        directory: row.get(1)?,
                        title: row.get(2)?,
                        created_at: row.get(3)?,
                        updated_at: row.get(4)?,
                        message_count: 0,
                    })
                },
            )
            .optional()?;
        let Some(row) = row else { return Ok(None) };

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
            current_model = Self::model_name(data.get("model")).or(current_model);
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

        let mut session = self.base_session(row);
        session.message_count = messages.len();
        session.last_message = messages
            .last()
            .and_then(|message| message.content.clone())
            .unwrap_or_default();
        Ok(Some(SessionDetail { session, messages }))
    }

    fn stats(&self) -> Result<SessionStats> {
        let connection = self.connection()?;
        let (total_sessions, first_session_date, last_session_date): (usize, Option<i64>, Option<i64>) = connection.query_row(
            "SELECT COUNT(*), MIN(time_created), MAX(time_updated) FROM session WHERE time_archived IS NULL", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let total_messages =
            connection.query_row("SELECT COUNT(*) FROM message", [], |row| row.get(0))?;
        Ok(SessionStats {
            total_sessions,
            total_messages,
            first_session_date,
            last_session_date,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
