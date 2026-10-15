use std::{collections::HashMap, fs, path::PathBuf, time::UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use walkdir::WalkDir;

use super::{CodeBuddyProvider, SessionProvider};
use crate::{AppType, Session, SessionDetail, model::SessionKind};

#[derive(Debug, Clone)]
pub struct WorkBuddyProvider {
    root: PathBuf,
}

impl Default for WorkBuddyProvider {
    fn default() -> Self {
        Self::with_root(dirs::home_dir().unwrap_or_default().join(".workbuddy"))
    }
}

impl WorkBuddyProvider {
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    fn load(&self, session: &Session) -> Result<Option<SessionDetail>> {
        let path = super::search_path(&self.root.join("projects"), &session.file_path)?;
        anyhow::ensure!(
            session.app_type == AppType::WorkBuddy
                && path.file_stem().and_then(|s| s.to_str()) == Some(session.id.as_str()),
            "Session path does not match its ID or provider"
        );
        let records: Vec<Value> = fs::read_to_string(path)?
            .lines()
            // A running session may end with an incomplete JSONL record.
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let mut messages =
            CodeBuddyProvider::with_root(self.root.clone()).normalize(&records, session.updated_at);
        for message in &mut messages {
            if message.sub_agent_session_id.is_some() {
                message.metadata.insert(
                    "childSessionAppType".into(),
                    Value::String("workbuddy".into()),
                );
            }
        }
        let mut session = session.clone();
        session.message_count = messages.len();
        Ok(Some(SessionDetail {
            session,
            messages,
            subtree_usage: None,
        }))
    }
}

impl SessionProvider for WorkBuddyProvider {
    fn app_type(&self) -> AppType {
        AppType::WorkBuddy
    }

    fn is_available(&self) -> bool {
        self.root.join("workbuddy.db").is_file()
    }

    fn sessions(&self) -> Result<Vec<Session>> {
        if !self.is_available() {
            return Ok(Vec::new());
        }
        let db = super::search_path(&self.root, &self.root.join("workbuddy.db"))?;
        let connection = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        // Main titles come from SQLite; subagents read only a bounded transcript prefix.
        let projects = self.root.join("projects");
        let mut files = HashMap::new();
        for entry in WalkDir::new(&projects)
            .min_depth(2)
            .max_depth(2)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.path();
            if path.extension().is_some_and(|s| s == "jsonl") {
                if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
                    files.insert(id.to_owned(), path.to_path_buf());
                }
            }
        }
        let mut query = connection.prepare(
            "SELECT id, cwd, COALESCE(NULLIF(custom_title, ''), NULLIF(title, ''), id),
                    created_at, updated_at
             FROM sessions WHERE deleted_at IS NULL AND transport = 'local'",
        )?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        let mut sessions = Vec::new();
        for row in rows {
            let (id, cwd, title, created_at, updated_at) = row?;
            let Some(path) = files.get(&id) else {
                continue;
            };
            let path = super::search_path(&projects, path)?;
            let parent = Session {
                id: id.clone(),
                app_type: AppType::WorkBuddy,
                file_name: title.clone(),
                file_path: path.clone(),
                created_at,
                updated_at,
                // The SQLite index has no message count; fill it when details are opened.
                message_count: 0,
                first_message: title.clone(),
                last_message: title,
                directory: (!cwd.is_empty()).then(|| PathBuf::from(cwd)),
                uuid: Some(id.clone()),
                kind: SessionKind::Main,
                parent_session_id: None,
                agent_type: None,
            };
            // Derive the subagent folder from the checked file, never from a database path.
            let children = path.with_extension("").join("subagents");
            if let Ok(entries) = fs::read_dir(children) {
                for entry in entries.flatten() {
                    let child_path = entry.path();
                    if !entry.file_type()?.is_file()
                        || child_path.extension().is_none_or(|s| s != "jsonl")
                    {
                        continue;
                    }
                    let child_path = super::search_path(&projects, &child_path)?;
                    let Some(child_id) = child_path.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    let mut child = parent.clone();
                    child.id = child_id.to_owned();
                    child.uuid = Some(child.id.clone());
                    child.file_name = child.id.clone();
                    child.last_message.clear();
                    child.first_message = CodeBuddyProvider::summary_values(&child_path)
                        .iter()
                        .filter(|record| {
                            record.get("type").and_then(Value::as_str) == Some("message")
                                && record.get("role").and_then(Value::as_str) == Some("user")
                        })
                        .find_map(|record| {
                            CodeBuddyProvider::message_text(record)
                                .lines()
                                .map(str::trim)
                                .find(|line| !line.is_empty())
                                .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
                        })
                        .unwrap_or_default()
                        .chars()
                        .take(100)
                        .collect();
                    child.kind = SessionKind::Subagent;
                    child.parent_session_id = Some(id.clone());
                    let metadata = entry.metadata()?;
                    let millis = |time: std::io::Result<std::time::SystemTime>| {
                        time.ok()
                            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                            .map(|duration| duration.as_millis() as i64)
                    };
                    child.created_at = millis(metadata.created()).unwrap_or(created_at);
                    child.updated_at = millis(metadata.modified()).unwrap_or(updated_at);
                    child.file_path = child_path;
                    sessions.push(child);
                }
            }
            sessions.push(parent);
        }
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        Ok(sessions)
    }

    fn session_detail(&self, session_id: &str) -> Result<Option<SessionDetail>> {
        match self
            .sessions()?
            .iter()
            .find(|session| session.id == session_id)
        {
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
    use crate::{MessageType, search::SearchScope};
    use serde_json::json;

    #[test]
    fn sqlite_listing_is_lazy_and_filters_deleted_cloud_and_missing_sessions() {
        let root = std::env::temp_dir().join(format!("yes-workbuddy-{}", std::process::id()));
        let project = root.join("projects/project");
        fs::create_dir_all(project.join("main/subagents")).unwrap();
        let db = Connection::open(root.join("workbuddy.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE sessions (id TEXT, cwd TEXT, title TEXT, custom_title TEXT,
                created_at INTEGER, updated_at INTEGER, deleted_at INTEGER, transport TEXT);
             INSERT INTO sessions VALUES ('main','/tmp/project','Generated','Custom',1000,2000,NULL,'local');
             INSERT INTO sessions VALUES ('deleted','/tmp/project','Deleted',NULL,1000,2000,3000,'local');
             INSERT INTO sessions VALUES ('cloud','/tmp/project','Cloud',NULL,1000,2000,NULL,'cloud');
             INSERT INTO sessions VALUES ('missing','/tmp/project','Missing',NULL,1000,2000,NULL,'local');
             INSERT INTO sessions VALUES ('../../outside','/tmp','Escape',NULL,1000,2000,NULL,'local');",
        ).unwrap();
        for id in ["main", "deleted", "cloud", "unindexed"] {
            // Main bodies are not needed for listing; invalid child prefixes are tolerated.
            fs::write(project.join(format!("{id}.jsonl")), [0xff]).unwrap();
        }
        let child_path = project.join("main/subagents/agent-child.jsonl");
        fs::write(&child_path, [0xff]).unwrap();
        let provider = WorkBuddyProvider::with_root(root.clone());
        assert!(provider.is_available());
        let sessions = provider.sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        let main = sessions.iter().find(|s| s.id == "main").unwrap();
        assert_eq!(main.file_name, "Custom");
        assert_eq!(main.first_message, "Custom");
        assert_eq!(
            main.directory.as_deref(),
            Some(std::path::Path::new("/tmp/project"))
        );
        assert_eq!((main.created_at, main.updated_at), (1000, 2000));
        assert_eq!(main.message_count, 0);
        let child = sessions.iter().find(|s| s.id == "agent-child").unwrap();
        assert_eq!(child.parent_session_id.as_deref(), Some("main"));
        assert_eq!(child.kind, SessionKind::Subagent);
        assert!(child.first_message.is_empty());
        assert_eq!(child.file_name, "agent-child");
        // Skip non-user and empty records, normalize whitespace, and preserve Unicode.
        let prompt = format!("\n  目标：检查\t业务埋点 {}\n详细要求", "中".repeat(120));
        let prefix = [
            json!({"type":"message","role":"assistant","content":"Not the task"}),
            json!({"type":"message","role":"user","content":"  \n"}),
            json!({"type":"message","role":"user","content":[{"type":"input_text","text":prompt}]}),
        ]
        .iter()
        .map(|record| format!("{record}\n"))
        .collect::<String>();
        // A large invalid tail must not prevent extracting a title from the prefix.
        fs::write(&child_path, prefix + &"x".repeat(512 * 1024)).unwrap();
        let titled = provider
            .sessions()
            .unwrap()
            .into_iter()
            .find(|session| session.id == "agent-child")
            .unwrap();
        assert!(titled.first_message.starts_with("目标：检查 业务埋点 中"));
        assert_eq!(titled.first_message.chars().count(), 100);
        assert!(!titled.first_message.contains('\n'));
        assert_eq!(titled.id, child.id);
        assert_eq!(
            provider
                .session_detail_from_summary(&titled)
                .unwrap()
                .unwrap()
                .session
                .first_message,
            titled.first_message
        );
        // Content beyond the prefix budget is not parsed for the list.
        fs::write(
            &child_path,
            " ".repeat(256 * 1024)
                + "\n"
                + &json!({"type":"message","role":"user","content":"Too late"}).to_string(),
        )
        .unwrap();
        assert!(
            provider
                .sessions()
                .unwrap()
                .iter()
                .find(|session| session.id == "agent-child")
                .unwrap()
                .first_message
                .is_empty()
        );
        fs::write(
            &child_path,
            json!({"type":"message","role":"user","content":"Review worker retries"}).to_string(),
        )
        .unwrap();
        assert_eq!(
            provider
                .sessions()
                .unwrap()
                .iter()
                .find(|session| session.id == "agent-child")
                .unwrap()
                .first_message,
            "Review worker retries"
        );
        assert!(provider.load(main).is_err());
        let records = [
            json!({"type":"message","role":"user","timestamp":1000,"content":[{"type":"input_text","text":"Find the needle"}]}),
            json!({"type":"reasoning","timestamp":1100,"rawContent":[{"type":"reasoning_text","text":"Thinking"}]}),
            json!({"type":"function_call","timestamp":1200,"name":"Read","callId":"call","arguments":"{\"path\":\"a.rs\"}","providerData":{"model":"test","messageId":"response","usage":{"inputTokens":100,"outputTokens":20,"totalTokens":120}}}),
            json!({"type":"function_call_result","timestamp":1300,"name":"Read","callId":"call","output":{"type":"text","text":"file body"}}),
            json!({"type":"message","role":"assistant","timestamp":1400,"content":[{"type":"output_text","text":"Answer"}]}),
            json!({"type":"function_call_result","timestamp":1600,"name":"Agent","callId":"agent-call","output":{"childSessionId":"agent-child"}}),
        ];
        fs::write(
            &main.file_path,
            records.iter().map(|r| format!("{r}\n")).collect::<String>() + "{\"type\":",
        )
        .unwrap();
        fs::write(&child_path, json!({"type":"message","role":"assistant","timestamp":1500,"content":[{"type":"output_text","text":"Child answer"}],"providerData":{"usage":{"inputTokens":50,"outputTokens":10,"totalTokens":60}}}).to_string()).unwrap();
        let detail = provider.session_detail_from_summary(main).unwrap().unwrap();
        assert_eq!(detail.session.app_type, AppType::WorkBuddy);
        assert_eq!(detail.session.file_name, "Custom");
        assert_eq!(detail.messages.len(), 5);
        assert_eq!(
            detail.messages[4].sub_agent_session_id.as_deref(),
            Some("agent-child")
        );
        assert_eq!(
            detail.messages[4].metadata["childSessionAppType"],
            "workbuddy"
        );
        assert_eq!(detail.messages[0].message_type, MessageType::User);
        assert_eq!(detail.messages[1].tool_name.as_deref(), Some("Read"));
        assert_eq!(
            detail.messages[1].reasoning_content.as_deref(),
            Some("Thinking")
        );
        assert_eq!(
            detail.messages[1].tool_input.as_ref().unwrap()["path"],
            "a.rs"
        );
        assert_eq!(detail.messages[1].call_id, detail.messages[2].call_id);
        assert_eq!(
            detail.messages[2]
                .tool_output
                .as_ref()
                .unwrap()
                .output
                .as_deref(),
            Some("file body")
        );
        assert_eq!(detail.messages[3].content.as_deref(), Some("Answer"));
        assert_eq!(
            detail,
            provider.session_detail_for_search(main).unwrap().unwrap()
        );
        assert_eq!(detail, provider.session_detail("main").unwrap().unwrap());
        assert!(
            !crate::search::search_session(&provider, main, "needle", SearchScope::All)
                .unwrap()
                .is_empty()
        );
        let detail = provider
            .session_detail_with_usage_from_sessions("main", &sessions)
            .unwrap()
            .unwrap();
        assert_eq!(detail.subtree_usage.unwrap().total_tokens, Some(180));
        // Neither listing nor opening is allowed to update the source database.
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            5
        );
        let mut invalid = main.clone();
        invalid.id = "wrong".into();
        assert!(provider.load(&invalid).is_err());
        let outside = root.join("outside.jsonl");
        fs::write(&outside, "{}").unwrap();
        invalid = main.clone();
        invalid.file_path = outside.clone();
        assert!(provider.load(&invalid).is_err());
        fs::remove_file(&main.file_path).unwrap();
        std::os::unix::fs::symlink(&outside, &main.file_path).unwrap();
        assert!(provider.load(main).is_err());
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn absent_database_is_unavailable_and_does_not_get_created() {
        let root =
            std::env::temp_dir().join(format!("yes-workbuddy-absent-{}", std::process::id()));
        let provider = WorkBuddyProvider::with_root(root.clone());
        assert!(!provider.is_available());
        assert!(provider.sessions().unwrap().is_empty());
        assert!(!root.exists());
    }
}
