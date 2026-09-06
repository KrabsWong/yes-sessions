use std::borrow::Cow;

use anyhow::anyhow;
use gpui_kit::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;
use yes_core::AppType;

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
struct ProviderAssets;

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(asset) = ProviderAssets::get(path) {
            return Ok(Some(asset.data));
        }
        gpui_kit::assets::Assets
            .load(path)
            .map_err(|_| anyhow!("could not find asset at path \"{path}\""))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut assets = gpui_kit::assets::Assets.list(path)?;
        assets.extend(
            ProviderAssets::iter()
                .filter(|asset| asset.starts_with(path))
                .map(SharedString::from),
        );
        Ok(assets)
    }
}

#[derive(Clone, Copy)]
pub enum ProviderIcon {
    CodeBuddy,
    Claude,
    OpenCode,
    Codex,
}

impl gpui_kit::component::IconNamed for ProviderIcon {
    fn path(self) -> SharedString {
        match self {
            Self::CodeBuddy => "icons/codebuddy.svg",
            Self::Claude => "icons/claude.svg",
            Self::OpenCode => "icons/opencode.svg",
            Self::Codex => "icons/openai.svg",
        }
        .into()
    }
}

impl From<AppType> for ProviderIcon {
    fn from(value: AppType) -> Self {
        match value {
            AppType::CodeBuddy => Self::CodeBuddy,
            AppType::Claude => Self::Claude,
            AppType::OpenCode => Self::OpenCode,
            AppType::Codex => Self::Codex,
        }
    }
}
