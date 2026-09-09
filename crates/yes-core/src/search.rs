use std::{
    ops::Range,
    path::{Component, Path, PathBuf},
};

use crate::{MessageType, Session, SessionMessage, SessionProvider};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchScope {
    #[default]
    All,
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub message: usize,
    pub message_type: MessageType,
    pub excerpt: String,
}

/// Metadata filters applied before loading session contents.
#[derive(Clone, Debug, Default)]
pub struct SearchFilter {
    /// Inclusive lower bound for `Session.updated_at`, in Unix milliseconds.
    pub updated_from: Option<i64>,
    /// Exclusive upper bound for `Session.updated_at`, in Unix milliseconds.
    pub updated_until: Option<i64>,
    pub directories: Vec<PathBuf>,
    pub include_subdirectories: bool,
}

impl SearchFilter {
    pub fn matches(&self, session: &Session) -> bool {
        if self
            .updated_from
            .is_some_and(|from| session.updated_at < from)
            || self
                .updated_until
                .is_some_and(|until| session.updated_at >= until)
        {
            return false;
        }
        if self.directories.is_empty() {
            return true;
        }
        let Some(directory) = session.directory.as_deref() else {
            return false;
        };
        let directory = normalize_path(directory);
        self.directories.iter().any(|selected| {
            let selected = normalize_path(selected);
            directory == selected
                || (self.include_subdirectories && directory.starts_with(selected))
        })
    }
}

// Compare lexical path components without resolving symlinks or reading the filesystem.
fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_some_and(|name| name != "..") {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

/// Search lightweight metadata without loading the session's messages.
pub fn search_session_metadata(session: &Session, query: &str) -> Option<String> {
    let query = query.trim().to_lowercase();
    [
        Some(session.first_message.as_str()),
        Some(session.last_message.as_str()),
        Some(session.file_name.as_str()),
        Some(session.id.as_str()),
        session.uuid.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find_map(|text| excerpt(text, &query))
    .or_else(|| {
        session
            .directory
            .as_ref()
            .and_then(|directory| excerpt(&directory.to_string_lossy(), &query))
    })
}

pub fn search_session(
    provider: &dyn SessionProvider,
    session: &Session,
    query: &str,
    scope: SearchScope,
) -> anyhow::Result<Vec<SearchHit>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let detail = provider
        .session_detail_for_search(session)?
        .ok_or_else(|| anyhow::anyhow!("Session was not found"))?;
    Ok(search_messages(&detail.messages, query, scope))
}

/// Locate an already-lowercased query and return byte offsets in the original text.
pub fn match_range(text: &str, query: &str) -> Option<Range<usize>> {
    if query.is_empty() {
        return None;
    }
    let offset = text.to_lowercase().find(query)?;
    let mut folded = 0;
    let mut start = None;
    for (index, ch) in text.char_indices() {
        let next = folded + ch.to_lowercase().map(char::len_utf8).sum::<usize>();
        if start.is_none() && next > offset {
            start = Some(index);
        }
        if next >= offset + query.len() {
            return Some(start?..index + ch.len_utf8());
        }
        folded = next;
    }
    None
}

// Return a small original-text excerpt, including when lowercasing changes UTF-8 length.
pub fn excerpt(text: &str, query: &str) -> Option<String> {
    let start = match_range(text, query)?.start;
    let prefix = text[..start]
        .char_indices()
        .rev()
        .nth(40)
        .map_or(0, |(i, _)| i);
    let body = text[prefix..]
        .chars()
        .take(200)
        .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
        .collect::<String>();
    Some(format!(
        "{}{}{}",
        if prefix > 0 { "…" } else { "" },
        body,
        if text[prefix..].chars().take(201).count() > 200 {
            "…"
        } else {
            ""
        }
    ))
}

pub fn search_messages(
    messages: &[SessionMessage],
    query: &str,
    scope: SearchScope,
) -> Vec<SearchHit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    messages
        .iter()
        .enumerate()
        .filter(|(_, item)| match scope {
            SearchScope::All => true,
            SearchScope::User => item.message_type == MessageType::User,
            SearchScope::Assistant => item.message_type == MessageType::Assistant,
        })
        .filter_map(|(message, item)| {
            let found = [
                item.content.as_deref(),
                item.reasoning_content.as_deref(),
                item.tool_name.as_deref(),
                item.tool_output
                    .as_ref()
                    .and_then(|out| out.output.as_deref()),
                item.tool_output
                    .as_ref()
                    .and_then(|out| out.preview.as_deref()),
            ]
            .into_iter()
            .flatten()
            .find_map(|text| excerpt(text, &query))
            .or_else(|| {
                item.attachments.iter().find_map(|attachment| {
                    excerpt(&attachment.name, &query).or_else(|| match &attachment.source {
                        crate::AttachmentSource::LocalPath(path) => {
                            excerpt(&path.to_string_lossy(), &query)
                        }
                        _ => None,
                    })
                })
            })
            .or_else(|| {
                item.tool_input.as_ref().and_then(|input| {
                    excerpt(
                        &serde_json::to_string_pretty(input).unwrap_or_default(),
                        &query,
                    )
                })
            });
            found.map(|excerpt| SearchHit {
                message,
                message_type: item.message_type,
                excerpt,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_names_and_paths_remain_searchable_without_indexing_payloads() {
        let mut message = SessionMessage::text(MessageType::User, "", "");
        message.attachments = vec![
            crate::SessionAttachment {
                name: "diagram.png".into(),
                embedded_fallback: None,
                mime_type: Some("image/png".into()),
                source: crate::AttachmentSource::LocalPath("/tmp/project/diagram.png".into()),
            },
            crate::SessionAttachment {
                name: "report.pdf".into(),
                embedded_fallback: None,
                mime_type: Some("application/pdf".into()),
                source: crate::AttachmentSource::DataUrl(
                    "data:application/pdf;base64,secretpayload".into(),
                ),
            },
        ];
        let messages = [message];
        for query in ["diagram", "project", "report"] {
            let hits = search_messages(&messages, query, SearchScope::User);
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].message, 0);
        }
        assert!(search_messages(&messages, "secretpayload", SearchScope::All).is_empty());
    }

    #[test]
    fn filters_message_sources_and_preserves_jump_indices() {
        let mut reasoning = SessionMessage::text(MessageType::Assistant, "", "");
        reasoning.reasoning_content = Some("needle in reasoning".into());
        let messages = vec![
            SessionMessage::text(MessageType::System, "", "needle"),
            SessionMessage::text(MessageType::User, "", "needle"),
            SessionMessage::text(MessageType::ToolUse, "", "needle"),
            SessionMessage::text(MessageType::Assistant, "", "needle"),
            SessionMessage::text(MessageType::ToolResult, "", "needle"),
            reasoning,
        ];
        let indices = |scope| {
            search_messages(&messages, "needle", scope)
                .into_iter()
                .map(|hit| hit.message)
                .collect::<Vec<_>>()
        };
        assert_eq!(indices(SearchScope::All), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(indices(SearchScope::User), vec![1]);
        assert_eq!(indices(SearchScope::Assistant), vec![3, 5]);
    }

    #[test]
    fn finds_literal_unicode_text_and_tool_input_once_per_message() {
        let mut tool = SessionMessage::text(crate::MessageType::ToolUse, "", "");
        tool.tool_input = Some(
            serde_json::json!({"path": "src/Test.rs"})
                .as_object()
                .unwrap()
                .clone(),
        );
        let messages = vec![
            SessionMessage::text(crate::MessageType::User, "", "中文 TEST test"),
            tool,
        ];
        assert_eq!(
            search_messages(&messages, "test", SearchScope::All).len(),
            2
        );
        assert_eq!(
            search_messages(&messages, "中文", SearchScope::All)[0].message,
            0
        );
        assert!(search_messages(&messages, "  ", SearchScope::All).is_empty());
        assert!(search_messages(&messages, "[.*]", SearchScope::All).is_empty());
        assert!(excerpt("İ Unicode 搜索", "搜索").unwrap().contains("搜索"));
    }
    #[test]
    fn bounds_long_message_excerpt() {
        let hits = search_messages(
            &[SessionMessage::text(
                crate::MessageType::User,
                "",
                format!("{}keyword{}", "长".repeat(100_000), "x".repeat(100_000)),
            )],
            "keyword",
            SearchScope::All,
        );
        assert_eq!(hits.len(), 1);
        assert!(hits[0].excerpt.contains("keyword"));
        assert!(hits[0].excerpt.chars().count() <= 202);
    }

    fn session(directory: Option<&str>, updated_at: i64) -> Session {
        Session {
            id: "fixture".into(),
            app_type: crate::AppType::Codex,
            file_name: "fixture".into(),
            file_path: "/tmp/fixture".into(),
            created_at: 0,
            updated_at,
            message_count: 0,
            first_message: String::new(),
            last_message: String::new(),
            directory: directory.map(PathBuf::from),
            uuid: None,
            kind: crate::model::SessionKind::Main,
            parent_session_id: None,
            agent_type: None,
        }
    }

    #[test]
    fn filters_time_boundaries_and_intersects_directory_union() {
        let filter = SearchFilter {
            updated_from: Some(1_000),
            updated_until: Some(2_000),
            directories: vec!["/work/a".into(), "/work/b".into()],
            include_subdirectories: false,
        };
        assert!(filter.matches(&session(Some("/work/a"), 1_000)));
        assert!(filter.matches(&session(Some("/work/b"), 1_999)));
        assert!(!filter.matches(&session(Some("/work/a"), 999)));
        assert!(!filter.matches(&session(Some("/work/a"), 2_000)));
        assert!(!filter.matches(&session(Some("/work/c"), 1_500)));
        assert!(!filter.matches(&session(None, 1_500)));
        assert!(SearchFilter::default().matches(&session(None, 1_500)));
    }

    #[test]
    fn directory_matching_uses_normalized_components() {
        let mut filter = SearchFilter {
            directories: vec!["/work/./a/../project/".into()],
            ..Default::default()
        };
        assert!(filter.matches(&session(Some("/work/project"), 0)));
        assert!(!filter.matches(&session(Some("/work/project/src"), 0)));
        filter.include_subdirectories = true;
        assert!(filter.matches(&session(Some("/work/project/src"), 0)));
        assert!(!filter.matches(&session(Some("/work/project-other"), 0)));
        assert!(!filter.matches(&session(Some("/work/project/../other"), 0)));
        assert_eq!(normalize_path(Path::new("/../../work")), Path::new("/work"));
        assert_eq!(
            normalize_path(Path::new("../../work")),
            Path::new("../../work")
        );
    }

    #[test]
    fn missing_session_is_a_search_failure() {
        struct Missing;
        impl SessionProvider for Missing {
            fn app_type(&self) -> crate::AppType {
                crate::AppType::Codex
            }
            fn is_available(&self) -> bool {
                true
            }
            fn sessions(&self) -> anyhow::Result<Vec<Session>> {
                Ok(Vec::new())
            }
            fn session_detail(&self, _: &str) -> anyhow::Result<Option<crate::SessionDetail>> {
                Ok(None)
            }
        }
        assert!(search_session(&Missing, &session(None, 0), "needle", SearchScope::All).is_err());
        assert!(
            search_session(&Missing, &session(None, 0), " ", SearchScope::All)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn searches_session_metadata_case_insensitively() {
        let mut session = session(Some("/work/Project"), 0);
        session.first_message = "Build 中文".into();
        session.uuid = Some("unique-id".into());
        for query in ["BUILD", "中文", "project", "unique-id", "fixture"] {
            assert!(search_session_metadata(&session, query).is_some());
        }
        assert!(search_session_metadata(&session, "missing").is_none());
        assert!(search_session_metadata(&session, " ").is_none());
    }

    #[test]
    fn unicode_match_offsets_are_original_utf8_boundaries() {
        let text = "İ Unicode 搜索";
        let range = match_range(text, "搜索").unwrap();
        assert_eq!(&text[range], "搜索");
        let range = match_range(text, "i").unwrap();
        assert_eq!(&text[range], "İ");
        assert!(match_range(text, "").is_none());
    }
}
