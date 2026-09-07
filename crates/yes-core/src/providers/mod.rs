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
