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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMessage {
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
    use super::AppType;

    #[test]
    fn provider_labels_match_the_desktop_ui() {
        assert_eq!(AppType::CodeBuddy.display_name(), "Codebuddy");
        assert_eq!(AppType::Claude.display_name(), "Claude Code");
        assert_eq!(AppType::OpenCode.display_name(), "OpenCode");
        assert_eq!(AppType::Codex.display_name(), "Codex CLI");
    }
}
