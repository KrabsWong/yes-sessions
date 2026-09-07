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
pub enum AccentColor {
    #[default]
    Default,
    Pink,
    Rose,
    Red,
    Orange,
    Amber,
    Yellow,
    Lime,
    Green,
    Emerald,
    Teal,
    Cyan,
    Sky,
    Blue,
    Indigo,
    Violet,
    Purple,
    Fuchsia,
    Slate,
    Zinc,
    Neutral,
}

impl AccentColor {
    pub const ALL: [Self; 21] = [
        Self::Default,
        Self::Pink,
        Self::Rose,
        Self::Red,
        Self::Orange,
        Self::Amber,
        Self::Yellow,
        Self::Lime,
        Self::Green,
        Self::Emerald,
        Self::Teal,
        Self::Cyan,
        Self::Sky,
        Self::Blue,
        Self::Indigo,
        Self::Violet,
        Self::Purple,
        Self::Fuchsia,
        Self::Slate,
        Self::Zinc,
        Self::Neutral,
    ];

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Pink => "Pink",
            Self::Rose => "Rose",
            Self::Red => "Red",
            Self::Orange => "Orange",
            Self::Amber => "Amber",
            Self::Yellow => "Yellow",
            Self::Lime => "Lime",
            Self::Green => "Green",
            Self::Emerald => "Emerald",
            Self::Teal => "Teal",
            Self::Cyan => "Cyan",
            Self::Sky => "Sky",
            Self::Blue => "Blue",
            Self::Indigo => "Indigo",
            Self::Violet => "Violet",
            Self::Purple => "Purple",
            Self::Fuchsia => "Fuchsia",
            Self::Slate => "Slate",
            Self::Zinc => "Zinc",
            Self::Neutral => "Neutral",
        }
    }
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
    pub accent_color: AccentColor,
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
            accent_color: AccentColor::Default,
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
        if let Some(settings) = fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
        {
            return settings;
        }

        let legacy_path = self
            .path
            .parent()
            .map(|parent| parent.join("yes-sessions-config.json"));
        let legacy_settings = legacy_path
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| value.get("settings").cloned())
            .and_then(|settings| serde_json::from_value(settings).ok());
        if let Some(settings) = legacy_settings {
            let _ = self.save(&settings);
            return settings;
        }

        AppSettings::default()
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
    fn accepts_settings_nested_in_the_legacy_electron_store_shape() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{"settings":{"language":"zh","theme":"dark","defaultApp":"codex"}}"#,
        )
        .unwrap();
        let settings: AppSettings =
            serde_json::from_value(value.get("settings").unwrap().clone()).unwrap();

        assert_eq!(settings.language, Language::Zh);
        assert_eq!(settings.theme, ThemePreference::Dark);
        assert_eq!(settings.default_app, Some(AppType::Codex));
    }
}
