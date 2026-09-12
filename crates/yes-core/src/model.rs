use std::{fmt, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppType {
    CodeBuddy,
    Claude,
    OpenCode,
    Codex,
}

impl AppType {
    pub const ALL: [Self; 4] = [Self::CodeBuddy, Self::Claude, Self::OpenCode, Self::Codex];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodeBuddy => "codebuddy",
            Self::Claude => "claude",
            Self::OpenCode => "opencode",
            Self::Codex => "codex",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::CodeBuddy => "Codebuddy",
            Self::Claude => "Claude Code",
            Self::OpenCode => "OpenCode",
            Self::Codex => "Codex CLI",
        }
    }
}

impl fmt::Display for AppType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AppType {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "codebuddy" => Ok(Self::CodeBuddy),
            "claude" => Ok(Self::Claude),
            "opencode" => Ok(Self::OpenCode),
            "codex" => Ok(Self::Codex),
            _ => Err(format!("unknown app type: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    User,
    Assistant,
    ToolUse,
    ToolResult,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentSource {
    LocalPath(PathBuf),
    DataUrl(String),
    RemoteUrl(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAttachment {
    pub name: String,
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedded_fallback: Option<String>,
    pub source: AttachmentSource,
}

impl SessionAttachment {
    pub fn is_image(&self) -> bool {
        self.mime_type
            .as_deref()
            .is_some_and(|mime| mime.starts_with("image/"))
            || std::path::Path::new(&self.name)
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| {
                    matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "png"
                            | "jpg"
                            | "jpeg"
                            | "gif"
                            | "webp"
                            | "bmp"
                            | "ico"
                            | "avif"
                            | "heic"
                            | "tiff"
                            | "tif"
                            | "svg"
                    )
                })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
}

impl TokenUsage {
    pub fn aggregate<'a>(usages: impl IntoIterator<Item = &'a Self>) -> Option<Self> {
        let mut usages = usages.into_iter();
        let mut total = usages.next()?.clone();
        for usage in usages {
            let add = |a: Option<u64>, b: Option<u64>| a?.checked_add(b?);
            total.input_tokens = add(total.input_tokens, usage.input_tokens);
            total.output_tokens = add(total.output_tokens, usage.output_tokens);
            total.total_tokens = add(total.total_tokens, usage.total_tokens);
            total.cache_read_tokens = add(total.cache_read_tokens, usage.cache_read_tokens);
        }
        Some(total)
    }

    pub fn cache_hit_rate(&self) -> Option<f64> {
        let input = self.input_tokens?;
        let cached = self.cache_read_tokens?;
        (input > 0 && cached <= input).then(|| cached as f64 / input as f64 * 100.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMessage {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<SessionAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    pub message_type: MessageType,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_agent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_output: Option<ToolOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl SessionMessage {
    pub fn text(
        message_type: MessageType,
        timestamp: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            attachments: Vec::new(),
            usage: None,
            message_type,
            timestamp: timestamp.into(),
            content: Some(content.into()),
            reasoning_content: None,
            redacted_content: None,
            sub_agent_session_id: None,
            tool_name: None,
            tool_input: None,
            tool_output: None,
            call_id: None,
            metadata: Map::new(),
            model: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    #[default]
    Main,
    Subagent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub app_type: AppType,
    pub file_name: String,
    pub file_path: PathBuf,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: usize,
    pub first_message: String,
    pub last_message: String,
    pub directory: Option<PathBuf>,
    pub uuid: Option<String>,
    pub kind: SessionKind,
    pub parent_session_id: Option<String>,
    pub agent_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionDetail {
    /// Usage of this session and its descendants, populated only for detail views.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtree_usage: Option<TokenUsage>,
    #[serde(flatten)]
    pub session: Session,
    pub messages: Vec<SessionMessage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionStats {
    pub total_sessions: usize,
    pub total_messages: usize,
    pub first_session_date: Option<i64>,
    pub last_session_date: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::{AppType, TokenUsage};

    #[test]
    fn usage_aggregate_preserves_unknowns_and_weights_cache_rate() {
        let first = TokenUsage {
            input_tokens: Some(100),
            output_tokens: Some(10),
            total_tokens: Some(110),
            cache_read_tokens: Some(100),
        };
        let second = TokenUsage {
            input_tokens: Some(900),
            output_tokens: None,
            total_tokens: None,
            cache_read_tokens: Some(0),
        };
        let total = TokenUsage::aggregate([&first, &second]).unwrap();
        assert_eq!(total.input_tokens, Some(1000));
        assert_eq!(total.output_tokens, None);
        assert_eq!(total.total_tokens, None);
        assert_eq!(total.cache_hit_rate(), Some(10.0));
        assert_eq!(TokenUsage::aggregate([]), None);
    }

    #[test]
    fn cache_rate_requires_valid_known_counts() {
        let mut usage = TokenUsage {
            input_tokens: Some(10000),
            cache_read_tokens: Some(9212),
            ..Default::default()
        };
        assert_eq!(format!("{:.2}%", usage.cache_hit_rate().unwrap()), "92.12%");
        usage.cache_read_tokens = Some(0);
        assert_eq!(usage.cache_hit_rate(), Some(0.0));
        usage.cache_read_tokens = None;
        assert_eq!(usage.cache_hit_rate(), None);
        usage.cache_read_tokens = Some(10001);
        assert_eq!(usage.cache_hit_rate(), None);
        usage.input_tokens = Some(0);
        usage.cache_read_tokens = Some(0);
        assert_eq!(usage.cache_hit_rate(), None);
        usage.input_tokens = None;
        assert_eq!(usage.cache_hit_rate(), None);
    }

    #[test]
    fn provider_labels_match_the_desktop_ui() {
        assert_eq!(AppType::CodeBuddy.display_name(), "Codebuddy");
        assert_eq!(AppType::Claude.display_name(), "Claude Code");
        assert_eq!(AppType::OpenCode.display_name(), "OpenCode");
        assert_eq!(AppType::Codex.display_name(), "Codex CLI");
    }
}
