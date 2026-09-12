mod claude;
mod codebuddy;
mod codex;
mod opencode;

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;

use crate::{AppType, Session, SessionDetail, SessionStats};

pub use claude::ClaudeProvider;
pub use codebuddy::CodeBuddyProvider;
pub use codex::CodexProvider;
pub use opencode::OpenCodeProvider;

pub trait SessionProvider: Send + Sync {
    fn app_type(&self) -> AppType;
    fn is_available(&self) -> bool;
    fn sessions(&self) -> Result<Vec<Session>>;
    fn session_detail(&self, session_id: &str) -> Result<Option<SessionDetail>>;

    /// Load a listed session for search without repeating provider-wide discovery.
    fn session_detail_for_search(&self, session: &Session) -> Result<Option<SessionDetail>> {
        self.session_detail(&session.id)
    }

    /// Load descendant usage only when opening details, leaving list/search reads lightweight.
    fn session_detail_with_usage(&self, session_id: &str) -> Result<Option<SessionDetail>> {
        let Some(mut detail) = self.session_detail(session_id)? else {
            return Ok(None);
        };
        let sessions = self.sessions()?;
        let mut pending = vec![detail.session.id.clone()];
        let mut visited = std::collections::HashSet::new();
        let mut usages = Vec::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id.clone()) {
                continue;
            }
            let child;
            let current = if id == detail.session.id {
                &detail
            } else {
                child = self.session_detail(&id)?;
                let Some(current) = child.as_ref() else {
                    continue;
                };
                if current.session.id != id && !visited.insert(current.session.id.clone()) {
                    continue;
                }
                current
            };
            usages.extend(
                current
                    .messages
                    .iter()
                    .filter_map(|message| message.usage.clone()),
            );
            pending.extend(
                current
                    .messages
                    .iter()
                    .filter_map(|message| message.sub_agent_session_id.clone()),
            );
            pending.extend(
                sessions
                    .iter()
                    .filter(|session| {
                        session.parent_session_id.as_deref() == Some(current.session.id.as_str())
                            || current.session.uuid.as_deref().is_some_and(|uuid| {
                                session.parent_session_id.as_deref() == Some(uuid)
                            })
                    })
                    .map(|session| session.id.clone()),
            );
        }
        detail.subtree_usage = crate::model::TokenUsage::aggregate(&usages);
        Ok(Some(detail))
    }

    fn stats(&self) -> Result<SessionStats> {
        let sessions = self.sessions()?;
        Ok(SessionStats {
            total_sessions: sessions.len(),
            total_messages: sessions.iter().map(|session| session.message_count).sum(),
            first_session_date: sessions.iter().map(|session| session.created_at).min(),
            last_session_date: sessions.iter().map(|session| session.updated_at).max(),
        })
    }
}

pub struct ProviderRegistry {
    providers: HashMap<AppType, Arc<dyn SessionProvider>>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        let mut registry = Self {
            providers: HashMap::new(),
        };
        registry.register(Arc::new(CodeBuddyProvider::default()));
        registry.register(Arc::new(ClaudeProvider::default()));
        registry.register(Arc::new(OpenCodeProvider::default()));
        registry.register(Arc::new(CodexProvider::default()));
        registry
    }
}

impl ProviderRegistry {
    pub fn register(&mut self, provider: Arc<dyn SessionProvider>) {
        self.providers.insert(provider.app_type(), provider);
    }
    pub fn get(&self, app_type: AppType) -> Option<Arc<dyn SessionProvider>> {
        self.providers.get(&app_type).cloned()
    }
}

// Search receives a cached path, so revalidate its canonical boundary before reading it.
fn search_path(root: &std::path::Path, path: &std::path::Path) -> Result<std::path::PathBuf> {
    let root = root.canonicalize()?;
    let path = path.canonicalize()?;
    anyhow::ensure!(
        path.starts_with(&root) && path.is_file(),
        "Session file is outside its provider root"
    );
    Ok(path)
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn search_loaders_preserve_messages_and_reject_external_paths() {
        let base =
            std::env::temp_dir().join(format!("yes-search-providers-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        let id = "22222222-2222-4222-8222-222222222222";
        let fixtures: Vec<(
            Box<dyn SessionProvider>,
            std::path::PathBuf,
            Vec<serde_json::Value>,
        )> = vec![
            (
                Box::new(ClaudeProvider::with_root(base.join("claude"))),
                base.join("claude/projects/project")
                    .join(format!("{id}.jsonl")),
                vec![
                    json!({"type":"user","uuid":"u","cwd":"/tmp/work","message":{"content":"needle"}}),
                ],
            ),
            (
                Box::new(CodeBuddyProvider::with_root(base.join("buddy"))),
                base.join("buddy/projects/Users-test-project")
                    .join(format!("{id}.jsonl")),
                vec![
                    json!({"type":"message","role":"user","sessionId":id,"cwd":"/tmp/work","content":[{"type":"input_text","text":"needle"}]}),
                ],
            ),
            (
                Box::new(CodexProvider::with_root(base.join("codex"))),
                base.join("codex/sessions")
                    .join(format!("rollout-2026-09-09T00-00-00-{id}.jsonl")),
                vec![
                    json!({"type":"session_meta","payload":{"id":id,"cwd":"/tmp/work"}}),
                    json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"needle"}]}}),
                ],
            ),
        ];
        for (provider, path, mut records) in fixtures {
            for record in &mut records {
                record["timestamp"] = json!("2026-09-09T00:00:00Z");
            }
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                &path,
                records.iter().map(|r| format!("{r}\n")).collect::<String>(),
            )
            .unwrap();
            let session = provider.sessions().unwrap().into_iter().next().unwrap();
            let expected = provider.session_detail(&session.id).unwrap().unwrap();
            let actual = provider
                .session_detail_for_search(&session)
                .unwrap()
                .unwrap();
            assert!(!actual.messages.is_empty());
            assert_eq!(actual.messages, expected.messages);
            let outside = base.join(format!("outside-{:?}.jsonl", provider.app_type()));
            fs::copy(&path, &outside).unwrap();
            let mut invalid = session.clone();
            invalid.file_path = outside.clone();
            assert!(provider.session_detail_for_search(&invalid).is_err());
            let link = path.with_file_name("escape.jsonl");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            invalid.file_path = link;
            assert!(provider.session_detail_for_search(&invalid).is_err());
            fs::remove_file(&path).unwrap();
            assert!(
                crate::search::search_session(
                    provider.as_ref(),
                    &session,
                    "needle",
                    crate::search::SearchScope::All
                )
                .is_err()
            );
        }
        fs::remove_dir_all(base).unwrap();
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;
    use crate::model::{MessageType, SessionKind, SessionMessage, TokenUsage};
    use std::sync::Mutex;

    struct Fixture {
        details: Vec<SessionDetail>,
        reads: Mutex<Vec<String>>,
    }

    impl SessionProvider for Fixture {
        fn app_type(&self) -> AppType {
            AppType::Codex
        }
        fn is_available(&self) -> bool {
            true
        }
        fn sessions(&self) -> Result<Vec<Session>> {
            Ok(self
                .details
                .iter()
                .filter(|detail| detail.session.id != "hidden")
                .map(|detail| detail.session.clone())
                .collect())
        }
        fn session_detail(&self, id: &str) -> Result<Option<SessionDetail>> {
            self.reads.lock().unwrap().push(id.to_owned());
            Ok(self
                .details
                .iter()
                .find(|detail| detail.session.id == id)
                .cloned())
        }
    }

    fn detail(id: &str, parent: Option<&str>, tokens: u64, links: &[&str]) -> SessionDetail {
        let mut message = SessionMessage::text(MessageType::Assistant, "", "response");
        message.usage = Some(TokenUsage {
            input_tokens: Some(tokens),
            output_tokens: Some(10),
            total_tokens: Some(tokens + 10),
            cache_read_tokens: Some(tokens / 2),
        });
        let mut messages = vec![message];
        for link in links {
            let mut message = SessionMessage::text(MessageType::ToolUse, "", "spawn");
            message.sub_agent_session_id = Some((*link).into());
            messages.push(message);
        }
        SessionDetail {
            subtree_usage: None,
            session: Session {
                id: id.into(),
                app_type: AppType::Codex,
                file_name: id.into(),
                file_path: id.into(),
                created_at: 0,
                updated_at: 0,
                message_count: messages.len(),
                first_message: String::new(),
                last_message: String::new(),
                directory: None,
                uuid: None,
                kind: if parent.is_some() {
                    SessionKind::Subagent
                } else {
                    SessionKind::Main
                },
                parent_session_id: parent.map(str::to_owned),
                agent_type: None,
            },
            messages,
        }
    }

    #[test]
    fn aggregates_nested_and_unlisted_children_once_without_changing_messages() {
        let root = detail("root", None, 100, &["child", "child", "hidden", "missing"]);
        let provider = Fixture {
            details: vec![
                root.clone(),
                detail("child", Some("root"), 200, &[]),
                detail("grandchild", Some("child"), 300, &["root"]),
                detail("hidden", Some("root"), 400, &[]),
                detail("unrelated", None, 9000, &[]),
            ],
            reads: Mutex::new(Vec::new()),
        };
        let result = provider.session_detail_with_usage("root").unwrap().unwrap();
        assert_eq!(result.messages, root.messages);
        let usage = result.subtree_usage.unwrap();
        assert_eq!(usage.input_tokens, Some(1000));
        assert_eq!(usage.output_tokens, Some(40));
        assert_eq!(usage.total_tokens, Some(1040));
        assert_eq!(usage.cache_hit_rate(), Some(50.0));
        let reads = provider.reads.lock().unwrap();
        for id in ["root", "child", "grandchild", "hidden", "missing"] {
            assert_eq!(reads.iter().filter(|read| read.as_str() == id).count(), 1);
        }
        assert!(!reads.iter().any(|id| id == "unrelated"));
    }

    #[test]
    fn missing_usage_fields_remain_unknown_and_leaf_totals_are_unchanged() {
        let root = detail("root", None, 100, &[]);
        let mut child = detail("child", Some("root"), 200, &[]);
        child.messages[0].usage.as_mut().unwrap().cache_read_tokens = None;
        let provider = Fixture {
            details: vec![root, child],
            reads: Mutex::new(Vec::new()),
        };
        let root = provider.session_detail_with_usage("root").unwrap().unwrap();
        assert_eq!(root.subtree_usage.unwrap().cache_read_tokens, None);
        let child = provider
            .session_detail_with_usage("child")
            .unwrap()
            .unwrap();
        assert_eq!(
            child.subtree_usage.as_ref(),
            child.messages[0].usage.as_ref()
        );
    }
}
