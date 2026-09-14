use std::{fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::AppType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Zh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    Light,
    Dark,
    #[default]
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChatLayout {
    #[default]
    Left,
    Bubble,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PreferredTerminal {
    #[default]
    Auto,
    Ghostty,
    Kitty,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub language: Language,
    pub theme: ThemePreference,
    pub auto_start: bool,
    pub lightweight_mode: bool,
    pub default_app: Option<AppType>,
    pub collapse_bash_blocks: bool,
    pub enable_title_marquee: bool,
    pub show_thinking_content: bool,
    pub chat_layout: ChatLayout,
    pub sidebar_collapsed: bool,
    pub preferred_terminal: PreferredTerminal,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: Language::En,
            theme: ThemePreference::System,
            auto_start: false,
            lightweight_mode: false,
            default_app: None,
            collapse_bash_blocks: true,
            enable_title_marquee: false,
            show_thinking_content: true,
            chat_layout: ChatLayout::Left,
            sidebar_collapsed: false,
            preferred_terminal: PreferredTerminal::Auto,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("yes-sessions")
            .join("settings.json")
    }

    pub fn load(&self) -> AppSettings {
        fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, settings: &AppSettings) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(settings)?)?;
        fs::rename(temporary, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_use_current_defaults() {
        let settings: AppSettings = serde_json::from_str(r#"{"language":"zh"}"#).unwrap();
        assert_eq!(settings.language, Language::Zh);
        assert!(settings.collapse_bash_blocks);
        assert_eq!(settings.theme, ThemePreference::System);
    }

    #[test]
    fn legacy_accent_is_ignored_without_resetting_other_settings() {
        let settings: AppSettings = serde_json::from_str(
            r#"{"accentColor":"purple","language":"zh","theme":"dark","collapseBashBlocks":false}"#,
        )
        .unwrap();
        assert_eq!(settings.language, Language::Zh);
        assert_eq!(settings.theme, ThemePreference::Dark);
        assert!(!settings.collapse_bash_blocks);
        assert!(
            serde_json::to_value(&settings)
                .unwrap()
                .get("accentColor")
                .is_none()
        );
    }

    #[test]
    fn native_settings_load_save_and_invalid_file_defaults() {
        let root = std::env::temp_dir().join(format!(
            "yes-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = SettingsStore::new(root.join("settings.json"));
        assert_eq!(store.load().theme, ThemePreference::System);
        let settings = AppSettings {
            language: Language::Zh,
            theme: ThemePreference::Dark,
            default_app: Some(AppType::Codex),
            ..AppSettings::default()
        };
        store.save(&settings).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.language, Language::Zh);
        assert_eq!(loaded.theme, ThemePreference::Dark);
        assert_eq!(loaded.default_app, Some(AppType::Codex));
        fs::write(root.join("settings.json"), "invalid json").unwrap();
        assert_eq!(store.load().theme, ThemePreference::System);
        fs::remove_dir_all(root).unwrap();
    }
}
