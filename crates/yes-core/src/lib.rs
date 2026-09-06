pub mod git;
pub mod mermaid;
pub mod model;
pub mod providers;
pub mod settings;
pub mod terminal;

pub use model::{AppType, MessageType, Session, SessionDetail, SessionMessage, SessionStats};
pub use providers::{ProviderRegistry, SessionProvider};
pub use settings::{
    AccentColor, AppSettings, ChatLayout, Language, PreferredTerminal, SettingsStore,
    ThemePreference,
};
