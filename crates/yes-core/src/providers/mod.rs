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
