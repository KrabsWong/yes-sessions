use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use gpui_kit::base::StyledExt;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    message_scroller::{MessageScroller, MessageScrollerState},
    text::{TextView, TextViewStyle},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use yes_core::mermaid::{ContentSegment, split_mermaid_blocks};
use yes_core::{AppType, Language, MessageType, SessionMessage};

use crate::{app::YesSessions, app_assets::ProviderIcon, i18n::tr, mermaid::MermaidDiagram};

#[derive(Clone, Copy)]
pub struct ConversationOptions {
    pub language: Language,
    pub provider: AppType,
    pub is_subagent: bool,
    pub show_thinking: bool,
    pub chat_bubbles: bool,
    pub collapse_tool_blocks: bool,
}

#[derive(Clone)]
struct IndexedMessage {
    index: usize,
    message: SessionMessage,
}

#[derive(Clone, Default)]
struct ConversationTurn {
    messages: Vec<IndexedMessage>,
}

fn conversation_markdown(id: impl Into<ElementId>, markdown: impl Into<SharedString>) -> TextView {
    let mut style = TextViewStyle::default().inline_code(HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    });
    style.heading_base_font_size = px(15.);
    TextView::markdown(id, markdown)
        .style(style)
        .text_sm()
        .text_size(px(15.))
}

#[derive(Clone, Default)]
struct ToolPair {
    tool_use: Option<IndexedMessage>,
    tool_result: Option<IndexedMessage>,
}

fn tool_use_matches_result(tool_use: &IndexedMessage, tool_result: &IndexedMessage) -> bool {
    let use_message = &tool_use.message;
    let result_message = &tool_result.message;
    if use_message.message_type != MessageType::ToolUse
        || result_message.message_type != MessageType::ToolResult
    {
        return false;
    }
    if let Some(call_id) = result_message.call_id.as_deref() {
        return use_message.call_id.as_deref() == Some(call_id);
    }
    result_message
        .tool_name
        .as_deref()
        .is_some_and(|tool_name| use_message.tool_name.as_deref() == Some(tool_name))
}

// Legacy transcripts may omit IDs; an explicit mismatch must remain an orphan.
fn tool_ids_allow_fallback(tool_use: &IndexedMessage, tool_result: &IndexedMessage) -> bool {
    tool_use.message.call_id.is_none() || tool_result.message.call_id.is_none()
}

fn tool_use_name_matches_result(tool_use: &IndexedMessage, tool_result: &IndexedMessage) -> bool {
    tool_use.message.message_type == MessageType::ToolUse
        && tool_result.message.message_type == MessageType::ToolResult
        && tool_ids_allow_fallback(tool_use, tool_result)
        && tool_result
            .message
            .tool_name
            .as_deref()
            .is_some_and(|name| tool_use.message.tool_name.as_deref() == Some(name))
}

fn turn_has_matching_tool_use(turn: &ConversationTurn, tool_result: &IndexedMessage) -> bool {
    turn.messages.iter().any(|candidate| {
        tool_use_matches_result(candidate, tool_result)
            && !turn.messages.iter().any(|existing| {
                existing.message.message_type == MessageType::ToolResult
                    && existing.message.call_id.is_some()
                    && existing.message.call_id == candidate.message.call_id
            })
    })
}

fn turn_has_matching_tool_name(turn: &ConversationTurn, tool_result: &IndexedMessage) -> bool {
    turn.messages.iter().any(|candidate| {
        tool_use_name_matches_result(candidate, tool_result)
            && !turn.messages.iter().any(|existing| {
                existing.message.message_type == MessageType::ToolResult
                    && ((existing.message.call_id.is_some()
                        && existing.message.call_id == candidate.message.call_id)
                        || (existing.message.call_id.is_none()
                            && existing.message.tool_name == candidate.message.tool_name))
            })
    })
}

fn turn_has_unmatched_tool_use(turn: &ConversationTurn, tool_result: &IndexedMessage) -> bool {
    turn.messages.iter().any(|candidate| {
        candidate.message.message_type == MessageType::ToolUse
            && tool_ids_allow_fallback(candidate, tool_result)
            && !turn.messages.iter().any(|existing| {
                existing.message.message_type == MessageType::ToolResult
                    && ((existing.message.call_id.is_some()
                        && existing.message.call_id == candidate.message.call_id)
                        || (existing.message.call_id.is_none()
                            && existing.message.tool_name == candidate.message.tool_name))
            })
    })
}

fn build_turns(messages: &[SessionMessage], provider: AppType) -> Vec<ConversationTurn> {
    let mut turns = Vec::new();
    let mut current: Option<ConversationTurn> = None;

    for (index, message) in messages.iter().cloned().enumerate() {
        let item = IndexedMessage { index, message };
        match item.message.message_type {
            MessageType::User => {
                if let Some(turn) = current.take().filter(|turn| !turn.messages.is_empty()) {
                    turns.push(turn);
                }
                current = Some(ConversationTurn {
                    messages: vec![item],
                });
            }
            MessageType::System => current
                .get_or_insert_with(ConversationTurn::default)
                .messages
                .push(item),
            MessageType::ToolUse => {
                if let Some(turn) = current.as_mut() {
                    turn.messages.push(item);
                } else {
                    turns.push(ConversationTurn {
                        messages: vec![item],
                    });
                }
            }
            MessageType::ToolResult => {
                if let Some(turn) = current
                    .as_mut()
                    .filter(|turn| turn_has_matching_tool_use(turn, &item))
                {
                    turn.messages.push(item);
                } else if let Some(turn) = turns
                    .iter_mut()
                    .rev()
                    .find(|turn| turn_has_matching_tool_use(turn, &item))
                {
                    turn.messages.push(item);
                } else if let Some(turn) = current
                    .as_mut()
                    .filter(|turn| turn_has_matching_tool_name(turn, &item))
                {
                    turn.messages.push(item);
                } else if let Some(turn) = turns
                    .iter_mut()
                    .find(|turn| turn_has_matching_tool_name(turn, &item))
                {
                    turn.messages.push(item);
                } else if let Some(turn) = current
                    .as_mut()
                    .filter(|turn| turn_has_unmatched_tool_use(turn, &item))
                {
                    turn.messages.push(item);
                } else if let Some(turn) = turns
                    .iter_mut()
                    .rev()
                    .find(|turn| turn_has_unmatched_tool_use(turn, &item))
                {
                    turn.messages.push(item);
                } else if let Some(turn) = current.as_mut() {
                    turn.messages.push(item);
                } else {
                    turns.push(ConversationTurn {
                        messages: vec![item],
                    });
                }
            }
            MessageType::Assistant => {
                if let Some(turn) = current.as_mut() {
                    turn.messages.push(item);
                    let next = messages.get(index + 1);
                    let keep_open = provider == AppType::Claude
                        && next.is_some_and(|message| {
                            matches!(
                                message.message_type,
                                MessageType::Assistant | MessageType::ToolUse
                            )
                        });
                    if !keep_open {
                        turns.push(current.take().expect("current turn exists"));
                    }
                } else {
                    turns.push(ConversationTurn {
                        messages: vec![item],
                    });
                }
            }
        }
    }
    if let Some(turn) = current.filter(|turn| !turn.messages.is_empty()) {
        turns.push(turn);
    }
    turns
}

pub fn conversation_turn_count(messages: &[SessionMessage], provider: AppType) -> usize {
    build_turns(messages, provider).len()
}

pub fn turn_index_for_message(
    messages: &[SessionMessage],
    message_index: usize,
    provider: AppType,
) -> usize {
    build_turns(messages, provider)
        .iter()
        .position(|turn| turn.messages.iter().any(|item| item.index == message_index))
        .unwrap_or_default()
}

fn pair_tool_messages(items: &[IndexedMessage]) -> Vec<ToolPair> {
    let mut pairs = Vec::<ToolPair>::new();
    for item in items {
        match item.message.message_type {
            MessageType::ToolUse => pairs.push(ToolPair {
                tool_use: Some(item.clone()),
                tool_result: None,
            }),
            MessageType::ToolResult => {
                let call_id = item.message.call_id.as_deref();
                let tool_name = item.message.tool_name.as_deref();
                let matching_index =
                    call_id
                        .and_then(|call_id| {
                            pairs.iter().position(|pair| {
                                pair.tool_result.is_none()
                                    && pair
                                        .tool_use
                                        .as_ref()
                                        .and_then(|message| message.message.call_id.as_deref())
                                        == Some(call_id)
                            })
                        })
                        .or_else(|| {
                            tool_name.and_then(|tool_name| {
                                pairs.iter().position(|pair| {
                                    pair.tool_result.is_none()
                                        && pair.tool_use.as_ref().is_some_and(|tool_use| {
                                            tool_ids_allow_fallback(tool_use, item)
                                        })
                                        && pair.tool_use.as_ref().and_then(|message| {
                                            message.message.tool_name.as_deref()
                                        }) == Some(tool_name)
                                })
                            })
                        })
                        .or_else(|| {
                            pairs.iter().position(|pair| {
                                pair.tool_result.is_none()
                                    && pair.tool_use.as_ref().is_some_and(|tool_use| {
                                        tool_ids_allow_fallback(tool_use, item)
                                    })
                            })
                        });
                if let Some(index) = matching_index {
                    pairs[index].tool_result = Some(item.clone());
                } else {
                    pairs.push(ToolPair {
                        tool_use: None,
                        tool_result: Some(item.clone()),
                    });
                }
            }
            _ => {}
        }
    }
    pairs
}

fn display_time(timestamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .map(|value| {
            value
                .with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| timestamp.chars().take(16).collect())
}

pub(crate) fn display_datetime(timestamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .map(|value| {
            value
                .with_timezone(&chrono::Local)
                .format("%Y/%m/%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| timestamp.chars().take(16).collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolType {
    Mcp,
    Subagent,
    Plan,
    Filesystem,
    Search,
    Code,
    Generic,
}

fn tool_type(tool_name: &str) -> ToolType {
    let name = tool_name.to_lowercase();
    if name.contains("skill") || name.contains("mcp") {
        ToolType::Mcp
    } else if name.contains("agent") || name.contains("spawn") || name.contains("delegate") {
        ToolType::Subagent
    } else if name.contains("planmode") || name == "enterplanmode" || name == "exitplanmode" {
        ToolType::Plan
    } else if [
        "read",
        "write",
        "glob",
        "grep",
        "edit",
        "ls",
        "mkdir",
        "apply_patch",
    ]
    .contains(&name.as_str())
        || name.contains("file")
    {
        ToolType::Filesystem
    } else if ["search", "fetch", "curl", "web"]
        .iter()
        .any(|part| name.contains(part))
    {
        ToolType::Search
    } else if [
        "bash",
        "python",
        "node",
        "npm",
        "exec_command",
        "write_stdin",
        "shell",
    ]
    .contains(&name.as_str())
        || name.contains("command")
    {
        ToolType::Code
    } else {
        ToolType::Generic
    }
}

fn tool_display_name(tool_name: &str) -> String {
    if let Some(name) = tool_name.strip_prefix("mcp:") {
        return format!("MCP {name}");
    }
    match tool_name {
        "read" => "Read File".into(),
        "write" => "Write File".into(),
        "edit" => "Edit File".into(),
        "glob" => "Find Files".into(),
        "grep" => "Search Content".into(),
        "ls" => "List Directory".into(),
        "mkdir" => "Create Directory".into(),
        "bash" | "exec_command" => "Execute Command".into(),
        "write_stdin" => "Write Stdin".into(),
        "apply_patch" => "Apply Patch".into(),
        "skill" => "MCP Skill".into(),
        "EnterPlanMode" => "Enter Plan Mode".into(),
        "ExitPlanMode" => "Exit Plan Mode".into(),
        _ => {
            let mut chars = tool_name.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

fn input_string<'a>(
    input: &'a serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| input.get(*key).and_then(serde_json::Value::as_str))
}

fn tool_file_target(
    name: &str,
    input: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<(PathBuf, Option<usize>)> {
    if !matches!(
        name.to_ascii_lowercase().as_str(),
        "read" | "write" | "edit" | "multiedit" | "read_file" | "write_file" | "edit_file"
    ) {
        return None;
    }
    let input = input?;
    let path = input_string(input, &["file_path", "filePath", "path"])?;
    if path.trim().is_empty() || path.contains('\0') {
        return None;
    }
    let line = ["start_line", "line", "offset"]
        .iter()
        .find_map(|key| input.get(*key).and_then(serde_json::Value::as_u64))
        .and_then(|line| usize::try_from(line).ok())
        .filter(|line| *line > 0);
    Some((PathBuf::from(path), line))
}

/// Keep the filename and its two nearest directories when shortening a tool path.
fn compact_tool_path(path: &str, home: Option<&std::path::Path>) -> String {
    let display = home
        .filter(|home| !home.as_os_str().is_empty())
        .and_then(|home| std::path::Path::new(path).strip_prefix(home).ok())
        .map(|relative| {
            if relative.as_os_str().is_empty() {
                "~".to_owned()
            } else {
                format!("~/{}", relative.display())
            }
        })
        .unwrap_or_else(|| path.to_owned());
    let parts: Vec<_> = display.split('/').filter(|part| !part.is_empty()).collect();
    if display.chars().count() <= 64 || parts.len() <= 3 {
        return display;
    }
    let root = if display.starts_with("~/") {
        "~/"
    } else if display.starts_with('/') {
        "/"
    } else {
        ""
    };
    format!("{root}.../{}", parts[parts.len() - 3..].join("/"))
}

fn path_basename(path: &str) -> &str {
    path.rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(path)
}

fn tool_summary(
    tool_name: &str,
    input: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<String> {
    let input = input?;
    let name = tool_name.to_lowercase();
    match name.as_str() {
        "read" | "write" | "edit" => input_string(input, &["file_path", "path"])
            .map(path_basename)
            .map(str::to_owned),
        "glob" => input_string(input, &["pattern", "glob"]).map(str::to_owned),
        "grep" => input_string(input, &["pattern", "regex"]).map(|pattern| {
            let location = input_string(input, &["path", "file_path"])
                .map(|path| format!(" in {}", path_basename(path)))
                .unwrap_or_default();
            format!("\"{pattern}\"{location}")
        }),
        "bash" | "exec_command" => input_string(input, &["command", "cmd"]).map(|command| {
            let mut chars = command.chars();
            let prefix = chars.by_ref().take(50).collect::<String>();
            if chars.next().is_some() {
                format!("{prefix}...")
            } else {
                prefix
            }
        }),
        "ls" => input_string(input, &["dir", "directory", "path"])
            .map(path_basename)
            .map(str::to_owned),
        _ => None,
    }
}

fn format_tool_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Null => "null".into(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            serde_json::to_string(value).unwrap_or_default()
        }
    }
}

fn tool_input_rows(
    input: Option<&serde_json::Map<String, serde_json::Value>>,
    omit_edit_strings: bool,
) -> Vec<(String, String)> {
    input
        .into_iter()
        .flat_map(|input| input.iter())
        .filter(|(key, _)| {
            !omit_edit_strings
                || !matches!(
                    key.as_str(),
                    "old_string" | "new_string" | "oldString" | "newString"
                )
        })
        .map(|(key, value)| (key.clone(), format_tool_value(value)))
        .collect()
}

fn edit_strings<'a>(
    tool_name: &str,
    input: Option<&'a serde_json::Map<String, serde_json::Value>>,
) -> Option<(&'a str, &'a str)> {
    if !tool_name.eq_ignore_ascii_case("edit") && !tool_name.eq_ignore_ascii_case("edit_file") {
        return None;
    }
    let input = input?;
    let old = input
        .get("old_string")
        .and_then(serde_json::Value::as_str)
        .or_else(|| input.get("oldString").and_then(serde_json::Value::as_str))?;
    let new = input
        .get("new_string")
        .and_then(serde_json::Value::as_str)
        .or_else(|| input.get("newString").and_then(serde_json::Value::as_str))?;
    Some((old, new))
}

fn render_edit_diff(
    index: usize,
    diff: crate::edit_diff::EditDiff,
    language: Language,
    cx: &App,
) -> AnyElement {
    use crate::edit_diff::Kind;
    let added = hsla(0.40, 0.55, 0.43, 1.);
    let removed = hsla(0.96, 0.72, 0.56, 1.);
    div()
        .mt_2()
        .min_w_0()
        .border_1()
        .border_color(cx.theme().border)
        .rounded(px(4.))
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_2()
                .py_1()
                .child(tr(language, "message.editDiff"))
                .when(diff.complete_input, |header| {
                    header
                        .child(div().text_color(added).child(format!("+{}", diff.added)))
                        .child(
                            div()
                                .text_color(removed)
                                .child(format!("−{}", diff.removed)),
                        )
                }),
        )
        .when(diff.limited, |view| {
            view.child(
                div()
                    .px_2()
                    .py_1()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(language, "message.editDiffLimited")),
            )
        })
        .child(
            div()
                .id(("historical-edit-diff", index))
                .debug_selector(|| "historical-edit-diff".into())
                .max_h(px(360.))
                .overflow_y_scroll()
                .map(|mut view| {
                    view.style().restrict_scroll_to_axis = Some(true);
                    view
                })
                .font_family(cx.theme().mono_font_family.clone())
                .children(diff.rows.into_iter().map(|row| {
                    let (marker, color) = match row.kind {
                        Kind::Added => ("+", added),
                        Kind::Removed => ("−", removed),
                        Kind::Context | Kind::Omitted => (" ", cx.theme().muted_foreground),
                    };
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .items_start()
                        .px_2()
                        .gap_2()
                        .when(matches!(row.kind, Kind::Added | Kind::Removed), |view| {
                            view.bg(color.opacity(0.12))
                        })
                        .child(div().flex_none().text_color(color).child(marker))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_normal()
                                .child(if row.text.is_empty() {
                                    " ".into()
                                } else {
                                    row.text
                                })
                                .when(
                                    row.no_newline
                                        && matches!(row.kind, Kind::Added | Kind::Removed),
                                    |view| {
                                        view.child(
                                            div()
                                                .text_color(cx.theme().muted_foreground)
                                                .text_xs()
                                                .child(format!(
                                                    "\\ {}",
                                                    tr(language, "message.noFinalNewline")
                                                )),
                                        )
                                    },
                                ),
                        )
                })),
        )
        .into_any_element()
}

fn tool_output_text(message: Option<&SessionMessage>) -> Option<String> {
    let message = message?;
    if let Some(output) = message.tool_output.as_ref() {
        if let Some(text) = output.output.clone().filter(|text| !text.is_empty()) {
            return Some(text);
        }
        if let Some(text) = output.extra.get("content").and_then(|content| {
            content.as_array().map(|items| {
                items
                    .iter()
                    .filter(|item| {
                        item.get("type").and_then(|value| value.as_str()) == Some("text")
                    })
                    .filter_map(|item| item.get("text").and_then(|value| value.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }) {
            if !text.is_empty() {
                return Some(text);
            }
        }
        if let Some(preview) = output.preview.clone().filter(|text| !text.is_empty()) {
            return Some(preview);
        }
        return serde_json::to_string_pretty(output).ok();
    }
    message
        .content
        .clone()
        .filter(|content| !content.is_empty())
}

fn tool_icon(tool_type: ToolType) -> IconName {
    match tool_type {
        ToolType::Mcp => IconName::Network,
        ToolType::Subagent => IconName::Bot,
        ToolType::Plan => IconName::GalleryVerticalEnd,
        ToolType::Filesystem => IconName::FileText,
        ToolType::Search => IconName::Search,
        ToolType::Code => IconName::SquareTerminal,
        ToolType::Generic => IconName::Settings2,
    }
}

fn tool_color(tool_type: ToolType) -> Hsla {
    match tool_type {
        ToolType::Mcp => hsla(217. / 360., 0.91, 0.60, 1.),
        ToolType::Filesystem => hsla(142. / 360., 0.71, 0.45, 1.),
        ToolType::Search => hsla(45. / 360., 0.93, 0.47, 1.),
        ToolType::Code => hsla(25. / 360., 0.95, 0.53, 1.),
        ToolType::Subagent => hsla(271. / 360., 0.91, 0.65, 1.),
        ToolType::Plan => hsla(239. / 360., 0.84, 0.67, 1.),
        ToolType::Generic => hsla(215. / 360., 0.16, 0.47, 1.),
    }
}

fn message_content(
    item: &IndexedMessage,
    mermaid_views: &HashMap<(usize, usize), Entity<MermaidDiagram>>,
) -> AnyElement {
    let Some(content) = item
        .message
        .content
        .clone()
        .filter(|value| !value.is_empty())
    else {
        return div().into_any_element();
    };
    message_content_text(&content, item.index, "body", 0, mermaid_views, None)
}

fn message_content_text(
    content: &str,
    message_index: usize,
    part: &str,
    diagram_start: usize,
    mermaid_views: &HashMap<(usize, usize), Entity<MermaidDiagram>>,
    text_style: Option<gpui_kit::base::TextViewStyle>,
) -> AnyElement {
    let markdown_view = |id: ElementId, markdown: String| -> AnyElement {
        if let Some(style) = &text_style {
            gpui_kit::base::TextView::markdown(id, markdown)
                .style(style.clone())
                .text_sm()
                .text_size(px(15.))
                .into_any_element()
        } else {
            conversation_markdown(id, markdown).into_any_element()
        }
    };
    let mut body = div().v_flex().gap_2().min_w_0().w_full().text_sm();
    let mut diagram_index = diagram_start;
    for segment in split_mermaid_blocks(content) {
        match segment {
            ContentSegment::Markdown(markdown) => {
                body = body.child(markdown_view(
                    (
                        ElementId::from(("message-content", message_index)),
                        format!("{part}-{diagram_index}"),
                    )
                        .into(),
                    markdown,
                ));
            }
            ContentSegment::Mermaid(source) => {
                if let Some(diagram) = mermaid_views.get(&(message_index, diagram_index)) {
                    body = body.child(diagram.clone());
                } else {
                    body = body.child(markdown_view(
                        (
                            ElementId::from(("mermaid-fallback", message_index)),
                            format!("{part}-{diagram_index}"),
                        )
                            .into(),
                        format!("```mermaid\n{source}\n```"),
                    ));
                }
                diagram_index += 1;
            }
        }
    }
    body.into_any_element()
}

fn xml_file_markdown(content: &str) -> String {
    // A payload may itself contain fences or HTML. Keep it entirely inside code.
    let longest = content.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}text\n{content}\n{fence}")
}

fn assistant_content(
    turn_index: usize,
    item: &IndexedMessage,
    options: ConversationOptions,
    mermaid_views: &HashMap<(usize, usize), Entity<MermaidDiagram>>,
    owner: WeakEntity<YesSessions>,
    cx: &App,
) -> AnyElement {
    use yes_core::claude_xml::{ClaudeXmlSegment, parse_claude_xml};
    if options.provider != AppType::Claude {
        return message_content(item, mermaid_views);
    }
    let segments = parse_claude_xml(item.message.content.as_deref().unwrap_or_default());
    let card_indices: Vec<_> = segments
        .iter()
        .enumerate()
        .filter_map(|(i, segment)| (!matches!(segment, ClaudeXmlSegment::Text(_))).then_some(i))
        .collect();
    if card_indices.is_empty() {
        return message_content(item, mermaid_views);
    }
    let message_index = item.index;
    let mut body = div().v_flex().w_full().min_w_0().gap_2();
    if card_indices.len() > 1 {
        let mut actions = div().flex().justify_end().gap_2();
        for (expand, key) in [
            (true, "sessions.expandAll"),
            (false, "sessions.collapseAll"),
        ] {
            let owner = owner.clone();
            let indices = card_indices.clone();
            actions = actions.child(
                Button::new((ElementId::from(("xml-actions", message_index)), key))
                    .ghost()
                    .debug_selector(move || format!("xml-action-{expand}"))
                    .label(tr(options.language, key))
                    .text_size(px(12.))
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.toggle_claude_xml(
                                turn_index,
                                message_index,
                                indices.clone(),
                                expand,
                                cx,
                            );
                        });
                    }),
            );
        }
        body = body.child(actions);
    }
    for (index, segment) in segments.into_iter().enumerate() {
        let (path, content, entries) = match segment {
            ClaudeXmlSegment::Text(text) => {
                let original = item.message.content.as_deref().unwrap_or_default();
                let preceding = &original[..text.as_ptr() as usize - original.as_ptr() as usize];
                let diagram_start = split_mermaid_blocks(preceding)
                    .iter()
                    .filter(|segment| matches!(segment, ContentSegment::Mermaid(_)))
                    .count();
                body = body.child(message_content_text(
                    text,
                    message_index,
                    &format!("xml-{index}"),
                    diagram_start,
                    mermaid_views,
                    None,
                ));
                continue;
            }
            ClaudeXmlSegment::File { path, content } => (path, Some(content), None),
            ClaudeXmlSegment::Directory { path, entries } => (path, None, Some(entries)),
        };
        let expanded = owner
            .upgrade()
            .and_then(|owner| {
                owner
                    .read(cx)
                    .claude_xml_expanded
                    .get(&(message_index, index))
                    .copied()
            })
            .unwrap_or(index == card_indices[0]);
        let directory = entries.is_some();
        let toggle_owner = owner.clone();
        let mut card = div()
            .w_full()
            .min_w_0()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .child(
                Button::new((
                    ElementId::from(("xml-card", message_index)),
                    index.to_string(),
                ))
                .ghost()
                .w_full()
                .h(px(36.))
                .px_3()
                .rounded_none()
                .debug_selector(move || format!("xml-card-{message_index}-{index}"))
                .accessibility_label(path.to_owned())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .child(
                            Icon::new(if directory {
                                IconName::Folder
                            } else {
                                IconName::FileText
                            })
                            .size(px(14.)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(path_basename(path.trim_end_matches('/')).to_owned()),
                        )
                        .child(
                            div()
                                .max_w(px(160.))
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child(compact_tool_path(
                                    path,
                                    std::env::var_os("HOME").map(PathBuf::from).as_deref(),
                                )),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child(tr(
                                    options.language,
                                    if directory {
                                        "sessions.directory"
                                    } else {
                                        "message.xmlFile"
                                    },
                                )),
                        )
                        .child(
                            Icon::new(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(px(14.)),
                        ),
                )
                .on_click(move |_, _, cx| {
                    let _ = toggle_owner.update(cx, |this, cx| {
                        this.toggle_claude_xml(
                            turn_index,
                            message_index,
                            vec![index],
                            !expanded,
                            cx,
                        );
                    });
                }),
            );
        if expanded {
            let mut payload = div()
                .id((
                    ElementId::from(("xml-body", message_index)),
                    index.to_string(),
                ))
                .debug_selector(move || format!("xml-body-{message_index}-{index}"))
                .w_full()
                .min_w_0()
                .max_h(px(360.))
                .overflow_y_scroll()
                .border_t_1()
                .border_color(cx.theme().border)
                .p_3();
            if let Some(content) = content {
                payload = payload.child(conversation_markdown(
                    (
                        ElementId::from(("xml-code", message_index)),
                        index.to_string(),
                    ),
                    xml_file_markdown(content),
                ));
            }
            if let Some(entries) = entries {
                let mut listing = div().v_flex().gap_1().child(
                    div()
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} {}",
                            entries.len(),
                            tr(options.language, "message.xmlEntries")
                        )),
                );
                for entry in entries {
                    listing = listing.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_sm()
                            .child(
                                Icon::new(if entry.ends_with('/') {
                                    IconName::Folder
                                } else {
                                    IconName::FileText
                                })
                                .size(px(14.)),
                            )
                            .child(div().min_w_0().child(entry.to_owned())),
                    );
                }
                payload = payload.child(listing);
            }
            card = card.child(payload);
        }
        body = body.child(card);
    }
    body.into_any_element()
}

fn avatar(icon: impl Into<Icon>, foreground: Hsla, background: Hsla) -> AnyElement {
    div()
        .flex_none()
        .size(px(32.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(background)
        .text_color(foreground)
        .child(Icon::new(icon).size(px(16.)))
        .into_any_element()
}

fn render_system(item: &IndexedMessage, options: ConversationOptions, cx: &App) -> AnyElement {
    div()
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().button)
        .overflow_hidden()
        .child(
            div()
                .px_3()
                .py(px(6.))
                .flex()
                .items_center()
                .justify_between()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(IconName::Info).size(px(14.)))
                        .child(tr(options.language, "message.system")),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground.opacity(0.6))
                        .child(display_time(&item.message.timestamp)),
                ),
        )
        .child(
            div()
                .px_3()
                .pb_2()
                .text_sm()
                .opacity(0.8)
                .child(message_content(item, &HashMap::new())),
        )
        .into_any_element()
}

fn render_reasoning(
    turn_index: usize,
    item: &IndexedMessage,
    options: ConversationOptions,
    expanded: &HashSet<usize>,
    owner: WeakEntity<YesSessions>,
    cx: &App,
) -> AnyElement {
    let content = item.message.reasoning_content.clone().unwrap_or_default();
    if !options.show_thinking || content.is_empty() {
        return div().into_any_element();
    }
    let message_index = item.index;
    let is_expanded = expanded.contains(&message_index);
    div()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().warning.opacity(0.35))
        .bg(cx.theme().warning.opacity(0.08))
        .overflow_hidden()
        .child(
            Button::new(("thinking", message_index))
                .ghost()
                .w_full()
                .h(px(34.))
                .px_3()
                .accessibility_label(tr(options.language, "message.thinking"))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(px(12.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_color(cx.theme().warning)
                                .child(Icon::new(IconName::Asterisk).size(px(14.)))
                                .child(tr(options.language, "message.thinking")),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_color(cx.theme().muted_foreground)
                                .child(if is_expanded {
                                    tr(options.language, "message.collapse")
                                } else {
                                    tr(options.language, "message.expand")
                                })
                                .child(
                                    Icon::new(if is_expanded {
                                        IconName::ChevronUp
                                    } else {
                                        IconName::ChevronDown
                                    })
                                    .size(px(14.)),
                                ),
                        ),
                )
                .on_click(move |_, _, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        this.toggle_message(message_index, turn_index, cx)
                    });
                }),
        )
        .when(is_expanded, |view| {
            view.child(
                div()
                    .px_3()
                    .pb_3()
                    .pt_2()
                    .text_sm()
                    .child(conversation_markdown(("reasoning", item.index), content)),
            )
        })
        .into_any_element()
}

fn render_tool(
    turn_index: usize,
    tool_use: Option<&IndexedMessage>,
    tool_result: Option<&IndexedMessage>,
    options: ConversationOptions,
    expanded: &HashSet<usize>,
    owner: WeakEntity<YesSessions>,
    cx: &App,
) -> AnyElement {
    let Some(item) = tool_use.or(tool_result) else {
        return div().into_any_element();
    };
    let is_expanded = if options.collapse_tool_blocks {
        expanded.contains(&item.index)
    } else {
        !expanded.contains(&item.index)
    };
    let tool_name = tool_use
        .and_then(|item| item.message.tool_name.as_deref())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            tool_result
                .and_then(|item| item.message.tool_name.as_deref())
                .filter(|name| !name.is_empty())
        })
        .unwrap_or("unknown")
        .to_owned();
    let title = tool_display_name(&tool_name);
    let kind = tool_type(&tool_name);
    let model = tool_use
        .and_then(|item| item.message.model.as_deref())
        .filter(|model| !model.is_empty())
        .or_else(|| {
            tool_result
                .and_then(|item| item.message.model.as_deref())
                .filter(|model| !model.is_empty())
        })
        .unwrap_or_default()
        .to_owned();
    let timestamp = tool_use
        .map(|item| item.message.timestamp.as_str())
        .filter(|timestamp| !timestamp.is_empty())
        .or_else(|| {
            tool_result
                .map(|item| item.message.timestamp.as_str())
                .filter(|timestamp| !timestamp.is_empty())
        })
        .map(display_datetime)
        .unwrap_or_default();
    let summary = tool_summary(
        &tool_name,
        tool_use.and_then(|item| item.message.tool_input.as_ref()),
    );
    let input = tool_use.and_then(|item| item.message.tool_input.as_ref());
    let file_target = tool_file_target(&tool_name, input).filter(|_| {
        owner
            .upgrade()
            .is_some_and(|owner| owner.read(cx).has_workspace())
    });
    let preview_owner = owner.clone();
    let chevron_owner = owner.clone();
    let summary = summary.filter(|_| file_target.is_none());
    let edit = is_expanded
        .then(|| edit_strings(&tool_name, input))
        .flatten();
    let input_rows = if is_expanded {
        tool_input_rows(input, edit.is_some())
    } else {
        Vec::new()
    };
    let edit_diff = edit.map(|(old, new)| crate::edit_diff::compare(old, new));
    let output = tool_output_text(tool_result.map(|item| &item.message))
        .or_else(|| tool_output_text(tool_use.map(|item| &item.message)));
    let message_index = item.index;
    let missing_input_label = if tool_use.is_none() {
        format!(" · {}", tr(options.language, "message.noInput"))
    } else {
        String::new()
    };
    let accessibility_label = format!(
        "{} · {} · {}{}",
        title,
        timestamp,
        if is_expanded {
            tr(options.language, "message.collapse")
        } else {
            tr(options.language, "message.expand")
        },
        missing_input_label,
    );
    div()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().list_active_border)
        .bg(cx.theme().muted.opacity(0.45))
        .overflow_hidden()
        .child(
            div()
                .id(("tool-header", message_index))
                .h(px(38.))
                .w_full()
                .flex()
                .items_center()
                .bg(cx.theme().button)
                .hover(|style| style.bg(cx.theme().accent))
                .child(
                    Button::new(("tool-toggle", message_index))
                        .custom(
                            ButtonCustomVariant::new(cx)
                                .color(cx.theme().transparent)
                                .foreground(cx.theme().foreground)
                                .hover(cx.theme().transparent)
                                .active(cx.theme().transparent),
                        )
                        .rounded_none()
                        .flex_1()
                        .min_w_0()
                        .h(px(38.))
                        .px_3()
                        .accessibility_label(accessibility_label)
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_sm()
                                .child(
                                    Icon::new(tool_icon(kind))
                                        .size(px(16.))
                                        .text_color(tool_color(kind)),
                                )
                                .child(
                                    div()
                                        .max_w(px(180.))
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(title),
                                )
                                .when(!model.is_empty(), |view| {
                                    view.child(
                                        div()
                                            .max_w(px(180.))
                                            .min_w_0()
                                            .truncate()
                                            .rounded(px(4.))
                                            .bg(cx.theme().muted)
                                            .px(px(6.))
                                            .py(px(2.))
                                            .text_size(px(12.))
                                            .text_color(cx.theme().foreground.opacity(0.78))
                                            .child(model),
                                    )
                                })
                                .child(
                                    div()
                                        .flex_none()
                                        .whitespace_nowrap()
                                        .text_size(px(12.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(timestamp),
                                )
                                .when_some(summary, |view, summary| {
                                    view.child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_right()
                                            .text_size(px(12.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(summary),
                                    )
                                })
                                .when(tool_use.is_none(), |view| {
                                    view.child(
                                        div()
                                            .flex_none()
                                            .text_size(px(10.))
                                            .text_color(cx.theme().warning.opacity(0.7))
                                            .child("※"),
                                    )
                                }),
                        )
                        .on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.toggle_message(message_index, turn_index, cx)
                            });
                        }),
                )
                .when_some(file_target, |view, (path, line)| {
                    let full_path = path.display().to_string();
                    let home = std::env::var_os("HOME").map(PathBuf::from);
                    let label = compact_tool_path(&full_path, home.as_deref());
                    let (directory, filename) = label
                        .rsplit_once('/')
                        .map(|(directory, filename)| (format!("{directory}/"), filename.to_owned()))
                        .unwrap_or_else(|| (String::new(), label));
                    view.child(
                        Button::new(("tool-file-preview", message_index))
                            .ghost()
                            .max_w(relative(0.6))
                            .min_w_0()
                            .h(px(38.))
                            .px_2()
                            .accessibility_label(full_path.clone())
                            .tooltip(full_path)
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(div().min_w_0().truncate().child(directory))
                                    .child(div().flex_none().whitespace_nowrap().child(filename)),
                            )
                            .on_click(move |_, window, cx| {
                                let _ = preview_owner.update(cx, |this, cx| {
                                    this.open_workspace_file(path.clone(), line, window, cx);
                                });
                            }),
                    )
                })
                .child(
                    Button::new(("tool-chevron-toggle", message_index))
                        .custom(
                            ButtonCustomVariant::new(cx)
                                .color(cx.theme().transparent)
                                .foreground(cx.theme().foreground)
                                .hover(cx.theme().transparent)
                                .active(cx.theme().transparent),
                        )
                        .rounded_none()
                        .flex_none()
                        .h(px(38.))
                        .icon(if is_expanded {
                            IconName::ChevronUp
                        } else {
                            IconName::ChevronDown
                        })
                        .accessibility_label(tr(
                            options.language,
                            if is_expanded {
                                "message.collapse"
                            } else {
                                "message.expand"
                            },
                        ))
                        .on_click(move |_, _, cx| {
                            let _ = chevron_owner.update(cx, |this, cx| {
                                this.toggle_message(message_index, turn_index, cx)
                            });
                        }),
                ),
        )
        .when(is_expanded && input.is_some(), |view| {
            view.child(
                div()
                    .border_t_1()
                    .border_dashed()
                    .border_color(cx.theme().list_active_border)
                    .px_3()
                    .py_2()
                    .text_size(px(12.))
                    .child(
                        div()
                            .mb_1()
                            .text_color(cx.theme().muted_foreground)
                            .child(tr(options.language, "message.input")),
                    )
                    .when(input_rows.is_empty() && edit_diff.is_none(), |section| {
                        section.child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(tr(options.language, "message.noInput")),
                        )
                    })
                    .when_some(edit_diff, |section, diff| {
                        section.child(render_edit_diff(message_index, diff, options.language, cx))
                    })
                    .children(input_rows.into_iter().map(|(key, value)| {
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_start()
                            .gap_2()
                            .py(px(2.))
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{key}:")),
                            )
                            .child(div().flex_1().min_w_0().whitespace_normal().child(value))
                    })),
            )
        })
        .when(is_expanded && output.is_some(), |view| {
            view.child(
                div()
                    .border_t_1()
                    .border_color(cx.theme().list_active_border.opacity(0.65))
                    .px_3()
                    .py_2()
                    .text_size(px(12.))
                    .child(
                        div()
                            .mb_1()
                            .text_color(cx.theme().muted_foreground)
                            .child(tr(options.language, "message.output")),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .rounded(px(4.))
                            .bg(cx.theme().muted.opacity(0.72))
                            .p_2()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(cx.theme().foreground.opacity(0.72))
                            .whitespace_normal()
                            .child(output.unwrap_or_default()),
                    ),
            )
        })
        .into_any_element()
}

fn subagent_string(
    input: Option<&serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Option<String> {
    input?
        .get(key)?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[allow(clippy::too_many_arguments)]
fn render_subagent(
    turn_index: usize,
    tool_use: Option<&IndexedMessage>,
    tool_result: Option<&IndexedMessage>,
    options: ConversationOptions,
    expanded: &HashSet<usize>,
    owner: WeakEntity<YesSessions>,
    cx: &App,
) -> Option<AnyElement> {
    let item = tool_use.or(tool_result)?;
    let tool_name = tool_use
        .and_then(|item| item.message.tool_name.as_deref())
        .or_else(|| tool_result.and_then(|item| item.message.tool_name.as_deref()))
        .unwrap_or("Agent");
    if tool_type(tool_name) != ToolType::Subagent {
        return None;
    }

    let input = tool_use.and_then(|item| item.message.tool_input.as_ref());
    let description = subagent_string(input, "description")
        .or_else(|| subagent_string(input, "task"))
        .or_else(|| subagent_string(input, "prompt"))
        .or_else(|| subagent_string(input, "message"))
        .unwrap_or_else(|| tr(options.language, "sessions.subAgentDefaultDesc").to_owned());
    let child_id = tool_result
        .and_then(|item| item.message.sub_agent_session_id.clone())
        .or_else(|| {
            tool_result.and_then(|item| {
                item.message
                    .metadata
                    .get("childSessionId")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            })
        })
        .or_else(|| tool_use.and_then(|item| item.message.sub_agent_session_id.clone()));
    let Some(child_id) = child_id else {
        // Older or incomplete logs may only contain a tool result. Keep it readable.
        return Some(render_tool(
            turn_index,
            tool_use,
            tool_result,
            options,
            expanded,
            owner,
            cx,
        ));
    };
    let status = tool_result
        .and_then(|item| item.message.metadata.get("subtype"))
        .or_else(|| tool_use.and_then(|item| item.message.metadata.get("subtype")))
        .and_then(|value| value.as_str())
        .unwrap_or(if tool_result.is_some() {
            "completed"
        } else {
            "running"
        });
    let (status_label, status_bg, status_fg) = match status {
        "completed" | "success" | "succeeded" => (
            tr(options.language, "sessions.completed"),
            hsla(
                142. / 360.,
                0.60,
                if cx.theme().mode.is_dark() {
                    0.20
                } else {
                    0.91
                },
                1.,
            ),
            hsla(
                142. / 360.,
                0.65,
                if cx.theme().mode.is_dark() {
                    0.68
                } else {
                    0.34
                },
                1.,
            ),
        ),
        "running" | "pending" | "in_progress" => (
            tr(options.language, "sessions.running"),
            hsla(
                43. / 360.,
                0.92,
                if cx.theme().mode.is_dark() {
                    0.20
                } else {
                    0.90
                },
                1.,
            ),
            hsla(
                38. / 360.,
                0.75,
                if cx.theme().mode.is_dark() {
                    0.68
                } else {
                    0.38
                },
                1.,
            ),
        ),
        _ => (
            tr(options.language, "sessions.failed"),
            hsla(
                0.,
                0.72,
                if cx.theme().mode.is_dark() {
                    0.20
                } else {
                    0.93
                },
                1.,
            ),
            hsla(
                0.,
                0.68,
                if cx.theme().mode.is_dark() {
                    0.70
                } else {
                    0.48
                },
                1.,
            ),
        ),
    };

    Some(
        div()
            .debug_selector(|| "subagent-summary".into())
            .w_full()
            .min_w_0()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().list_active_border)
            .bg(cx.theme().button)
            .p_3()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::Bot)
                            .size(px(16.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(tr(options.language, "sessions.subAgent")),
                    )
                    .child(
                        div()
                            .flex_none()
                            .rounded_full()
                            .bg(status_bg)
                            .px(px(6.))
                            .py(px(2.))
                            .text_size(px(10.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(status_fg)
                            .child(status_label),
                    )
                    .child(
                        div()
                            .debug_selector(|| "subagent-open-action".into())
                            .ml_auto()
                            .child(
                                Button::new(("sub-agent-open", item.index))
                                    .ghost()
                                    .compact()
                                    .h(px(26.))
                                    .text_size(px(12.))
                                    .text_color(cx.theme().foreground)
                                    .icon(IconName::ArrowRight)
                                    .label(tr(options.language, "sessions.viewSubAgentSession"))
                                    .on_click(move |_, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.open_sub_agent(&child_id, cx)
                                        });
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_sm()
                    .line_clamp(2)
                    .child(description),
            )
            .into_any_element(),
    )
}

fn render_user(
    item: &IndexedMessage,
    options: ConversationOptions,
    mermaid_views: &HashMap<(usize, usize), Entity<MermaidDiagram>>,
    cx: &App,
) -> AnyElement {
    let header = div()
        .flex()
        .items_center()
        .gap_2()
        .mb_1()
        .text_sm()
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().foreground)
                .when(options.is_subagent, |view| {
                    view.rounded(px(4.))
                        .px_2()
                        .py(px(2.))
                        .bg(hsla(270. / 360., 0.67, 0.42, 1.))
                        .text_color(rgb(0xffffff))
                })
                .child(tr(
                    options.language,
                    if options.is_subagent {
                        "message.mainAgent"
                    } else {
                        "message.user"
                    },
                )),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(display_time(&item.message.timestamp)),
        );
    let bubble = div()
        .rounded_lg()
        .bg(if options.is_subagent {
            cx.theme().button
        } else {
            cx.theme().primary
        })
        .text_color(if options.is_subagent {
            cx.theme().foreground
        } else {
            cx.theme().primary_foreground
        })
        .p_3()
        .text_sm()
        .when(options.chat_bubbles, |view| {
            view.border_1()
                .border_color(cx.theme().primary.opacity(0.2))
        })
        .child(
            if crate::large_message::needs_virtual_text(
                item.message.content.as_deref().unwrap_or_default(),
            ) {
                crate::large_message::LargeMessage {
                    id: item.index,
                    source: item.message.content.clone().unwrap_or_default().into(),
                    language: options.language,
                }
                .into_any_element()
            } else if options.is_subagent {
                message_content(item, mermaid_views)
            } else {
                let foreground = cx.theme().primary_foreground;
                let style = gpui_kit::base::TextViewStyle::default()
                    .with_foreground(foreground)
                    .with_muted_foreground(foreground.opacity(0.85))
                    .with_link(foreground)
                    .with_border(foreground.opacity(0.3))
                    .with_selection(foreground.opacity(0.25))
                    .with_heading_base_font_size(px(15.))
                    .with_code_background(cx.theme().muted)
                    .with_code_block(StyleRefinement::default().text_color(cx.theme().foreground))
                    .with_table_head(
                        StyleRefinement::default()
                            .bg(cx.theme().table_head)
                            .text_color(cx.theme().table_head_foreground),
                    )
                    .with_inline_code(HighlightStyle {
                        color: Some(foreground),
                        background_color: Some(foreground.opacity(0.12)),
                        font_weight: Some(FontWeight::BOLD),
                        ..Default::default()
                    })
                    .with_dark(cx.theme().mode.is_dark());
                message_content_text(
                    item.message.content.as_deref().unwrap_or_default(),
                    item.index,
                    "body",
                    0,
                    mermaid_views,
                    Some(style),
                )
            },
        );
    let user_avatar = avatar(
        if options.is_subagent {
            Icon::new(ProviderIcon::from(options.provider))
        } else {
            Icon::new(IconName::User)
        },
        cx.theme().foreground,
        cx.theme().button,
    );
    if options.chat_bubbles {
        div()
            .w_full()
            .flex()
            .justify_end()
            .items_start()
            .gap_3()
            .child(
                div()
                    .max_w(relative(0.85))
                    .min_w_0()
                    .v_flex()
                    .items_end()
                    .child(header)
                    .child(bubble),
            )
            .child(user_avatar)
            .into_any_element()
    } else {
        div()
            .w_full()
            .flex()
            .items_start()
            .gap_3()
            .child(user_avatar)
            .child(div().flex_1().min_w_0().child(header).child(bubble))
            .into_any_element()
    }
}

// Paint above the message without changing its measured size or intercepting input.
fn search_landing_feedback(
    index: usize,
    content: AnyElement,
    owner: &WeakEntity<YesSessions>,
    cx: &App,
) -> AnyElement {
    let landing = owner
        .upgrade()
        .and_then(|owner| owner.read(cx).search_landing);
    let Some((target, started)) = landing
        .filter(|(target, started)| *target == index && started.elapsed().as_secs_f32() < 4.)
    else {
        return content;
    };
    let should_scroll = owner
        .upgrade()
        .is_some_and(|owner| owner.read(cx).search_landing_scroll_pending);
    let scroll_owner = owner.clone();
    div()
        .debug_selector(move || format!("search-landing-{index}").into())
        .relative()
        .w_full()
        .min_w_0()
        .child(content)
        .child(
            canvas(
                move |bounds, window, _| {
                    if should_scroll {
                        window.request_autoscroll(Bounds {
                            origin: bounds.origin,
                            size: size(bounds.size.width, bounds.size.height.min(px(48.))),
                        });
                    }
                },
                move |_, _, _, cx| {
                    if should_scroll {
                        cx.defer(move |cx| {
                            let _ = scroll_owner.update(cx, |this, cx| {
                                if this.search_landing == Some((target, started)) {
                                    this.search_landing_scroll_pending = false;
                                    cx.notify();
                                }
                            });
                        });
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded(px(8.))
                .border_2()
                .with_animation(
                    SharedString::from(format!("search-landing-{target}-{started:?}")),
                    Animation::new(std::time::Duration::from_secs(4)),
                    move |view, _| {
                        let elapsed = started.elapsed().as_secs_f32();
                        let fade = ((4. - elapsed) / 0.8).clamp(0., 1.);
                        let pulse = 0.5 + 0.5 * (elapsed * std::f32::consts::TAU / 1.25).cos();
                        view.border_color(rgb(0xe8b521).opacity((0.35 + pulse * 0.6) * fade))
                            .bg(rgb(0xffd54f).opacity((0.035 + pulse * 0.075) * fade))
                    },
                ),
        )
        .into_any_element()
}

fn render_assistant_group(
    turn_index: usize,
    items: Vec<IndexedMessage>,
    options: ConversationOptions,
    expanded: &HashSet<usize>,
    mermaid_views: &HashMap<(usize, usize), Entity<MermaidDiagram>>,
    owner: WeakEntity<YesSessions>,
    cx: &App,
) -> AnyElement {
    let timestamp = items
        .first()
        .map(|item| display_time(&item.message.timestamp))
        .unwrap_or_default();
    let model = items
        .iter()
        .find_map(|item| item.message.model.clone())
        .unwrap_or_default();
    let mut sections = Vec::new();
    for pair in pair_tool_messages(&items) {
        let index = pair
            .tool_use
            .as_ref()
            .or(pair.tool_result.as_ref())
            .unwrap()
            .index;
        let mut body = div().v_flex().gap_2().min_w_0().w_full();
        if let Some(tool_use) = pair.tool_use.as_ref().filter(|item| {
            item.message
                .reasoning_content
                .as_deref()
                .is_some_and(|content| !content.is_empty())
        }) {
            body = body.child(render_reasoning(
                turn_index,
                tool_use,
                options,
                expanded,
                owner.clone(),
                cx,
            ));
        }
        if let Some(card) = render_subagent(
            turn_index,
            pair.tool_use.as_ref(),
            pair.tool_result.as_ref(),
            options,
            expanded,
            owner.clone(),
            cx,
        ) {
            body = body.child(card);
        } else {
            body = body.child(render_tool(
                turn_index,
                pair.tool_use.as_ref(),
                pair.tool_result.as_ref(),
                options,
                expanded,
                owner.clone(),
                cx,
            ));
        }
        let target = owner
            .upgrade()
            .and_then(|owner| owner.read(cx).search_landing)
            .map(|(index, _)| index);
        let feedback_index = pair
            .tool_result
            .as_ref()
            .filter(|item| Some(item.index) == target)
            .map_or(index, |item| item.index);
        sections.push((
            index,
            search_landing_feedback(feedback_index, body.into_any_element(), &owner, cx),
        ));
    }
    for item in items
        .iter()
        .filter(|item| item.message.message_type == MessageType::Assistant)
    {
        let body = div()
            .v_flex()
            .gap_2()
            .min_w_0()
            .w_full()
            .child(render_reasoning(
                turn_index,
                item,
                options,
                expanded,
                owner.clone(),
                cx,
            ))
            .when(
                item.message
                    .content
                    .as_deref()
                    .is_some_and(|content| !content.is_empty()),
                |view| {
                    view.child(assistant_content(
                        turn_index,
                        item,
                        options,
                        mermaid_views,
                        owner.clone(),
                        cx,
                    ))
                },
            );
        sections.push((
            item.index,
            search_landing_feedback(item.index, body.into_any_element(), &owner, cx),
        ));
    }
    // Claude interleaves explanatory text with tool blocks in the same turn.
    if options.provider == AppType::Claude {
        sections.sort_by_key(|(index, _)| *index);
    }
    let body = div()
        .v_flex()
        .gap_2()
        .min_w_0()
        .w_full()
        .children(sections.into_iter().map(|(index, element)| {
            div()
                .debug_selector(move || format!("assistant-section-{index}"))
                .w_full()
                .min_w_0()
                .child(element)
        }));
    div()
        .w_full()
        .flex()
        .items_start()
        .gap_3()
        .child(avatar(
            ProviderIcon::from(options.provider),
            cx.theme().foreground,
            cx.theme().button,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .v_flex()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .mb_1()
                        .text_sm()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(options.provider.display_name()),
                        )
                        .when(!model.is_empty(), |view| {
                            view.child(
                                div()
                                    .rounded(px(4.))
                                    .bg(cx.theme().muted)
                                    .px(px(6.))
                                    .py(px(2.))
                                    .text_size(px(12.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(model),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child(timestamp),
                        ),
                )
                .child(body),
        )
        .into_any_element()
}

fn render_turn(
    turn_index: usize,
    remeasure_turn_index: usize,
    turn: ConversationTurn,
    options: ConversationOptions,
    expanded: &HashSet<usize>,
    mermaid_views: &HashMap<(usize, usize), Entity<MermaidDiagram>>,
    owner: WeakEntity<YesSessions>,
    cx: &App,
) -> AnyElement {
    let systems = turn
        .messages
        .iter()
        .filter(|item| item.message.message_type == MessageType::System)
        .cloned()
        .collect::<Vec<_>>();
    let user = turn
        .messages
        .iter()
        .find(|item| item.message.message_type == MessageType::User)
        .cloned();
    let assistant = turn
        .messages
        .into_iter()
        .filter(|item| {
            matches!(
                item.message.message_type,
                MessageType::Assistant | MessageType::ToolUse | MessageType::ToolResult
            )
        })
        .collect::<Vec<_>>();
    div()
        .id(("conversation-turn", turn_index))
        .w_full()
        .v_flex()
        .gap_3()
        .children(systems.iter().map(|item| {
            search_landing_feedback(item.index, render_system(item, options, cx), &owner, cx)
        }))
        .when_some(user, |view, item| {
            view.child(search_landing_feedback(
                item.index,
                render_user(&item, options, mermaid_views, cx),
                &owner,
                cx,
            ))
        })
        .when(!assistant.is_empty(), |view| {
            view.child(render_assistant_group(
                remeasure_turn_index,
                assistant,
                options,
                expanded,
                mermaid_views,
                owner,
                cx,
            ))
        })
        .into_any_element()
}

pub fn conversation_scroller(
    messages: Arc<Vec<SessionMessage>>,
    state: Entity<MessageScrollerState>,
    options: ConversationOptions,
    expanded: HashSet<usize>,
    mermaid_views: Arc<HashMap<(usize, usize), Entity<MermaidDiagram>>>,
    owner: WeakEntity<YesSessions>,
) -> MessageScroller {
    let turns = Arc::new(build_turns(&messages, options.provider));
    let mut active_message = None;
    let anchors = turns
        .iter()
        .map(|turn| {
            for item in &turn.messages {
                if crate::app::is_navigable_user_message(&item.message) {
                    active_message = Some(item.index);
                }
            }
            active_message
        })
        .collect::<Vec<_>>();
    let scroll_state = state.clone();
    MessageScroller::new("conversation", state, move |index, _window, cx| {
        let Some(turn) = turns.get(index).cloned() else {
            return div().into_any_element();
        };
        let content = render_turn(
            index,
            index,
            turn,
            options,
            &expanded,
            &mermaid_views,
            owner.clone(),
            cx,
        );
        let anchor = anchors[index];
        let anchor_owner = owner.clone();
        let anchor_state = scroll_state.clone();
        div()
            .relative()
            .w_full()
            .child(content)
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        let viewport = window.content_mask().bounds;
                        let reading_edge = viewport.top() + px(12.);
                        let active = bounds.top() <= reading_edge && bounds.bottom() > reading_edge;
                        if active && bounds.intersects(&viewport) {
                            if let Some(message_index) = anchor {
                                cx.defer(move |cx| {
                                    let _ = anchor_owner.update(cx, |this, cx| {
                                        this.sync_navigator_message(
                                            message_index,
                                            &anchor_state,
                                            cx,
                                        );
                                    });
                                });
                            }
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .when(index + 1 == turns.len(), |view| {
                view.child(
                    div()
                        .debug_selector(|| "conversation-end".into())
                        .w_full()
                        .py_4()
                        .text_center()
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground)
                        .child(tr(options.language, "sessions.reachedBottom")),
                )
            })
            .into_any_element()
    })
    .with_jump_button_label(tr(options.language, "sessions.jumpLatest"))
}

#[cfg(test)]
mod tests {
    use super::{
        IndexedMessage, ToolType, build_turns, display_datetime, display_time, pair_tool_messages,
        tool_display_name, tool_input_rows, tool_summary, tool_type,
    };
    use serde_json::json;
    use yes_core::{AppType, MessageType, SessionMessage};

    struct EditDiffTestView;

    impl gpui_kit::Render for EditDiffTestView {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            cx: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            super::render_edit_diff(
                0,
                crate::edit_diff::compare("let old = 1;\n", "let new = 2;\n"),
                yes_core::Language::En,
                cx,
            )
        }
    }

    #[gpui_kit::test]
    fn historical_edit_diff_renders_in_a_narrow_message(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{px, size};
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(300.), px(400.)), |_, _| EditDiffTestView);
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("historical-edit-diff").unwrap();
        assert!(bounds.size.width > px(0.) && bounds.size.width <= px(300.));
        assert!(bounds.size.height > px(0.));
    }

    #[test]
    fn xml_code_payload_cannot_escape_its_fence() {
        let source = "```\n<script>text</script>\n````";
        assert_eq!(
            super::xml_file_markdown(source),
            format!("`````text\n{source}\n`````")
        );
    }

    #[test]
    fn edit_arguments_require_matching_tool_and_both_strings() {
        let snake = json!({"old_string":"before", "new_string":"after"});
        let camel = json!({"oldString":"", "newString":"added"});
        let mixed =
            json!({"old_string":null,"oldString":"old","new_string":"","newString":"ignored"});
        assert_eq!(
            super::edit_strings("edit", mixed.as_object()),
            Some(("old", ""))
        );
        assert_eq!(
            super::edit_strings("Edit", snake.as_object()),
            Some(("before", "after"))
        );
        assert_eq!(
            super::edit_strings("edit_file", camel.as_object()),
            Some(("", "added"))
        );
        assert_eq!(super::edit_strings("read", snake.as_object()), None);
        assert_eq!(
            super::edit_strings("edit", json!({"old_string":"old"}).as_object()),
            None
        );
        assert_eq!(
            super::edit_strings(
                "edit",
                json!({"old_string":1,"new_string":"new"}).as_object()
            ),
            None
        );
    }

    #[test]
    fn tool_paths_keep_filename_and_nearest_directories() {
        let home = Some(std::path::Path::new("/Users/krabswang"));
        assert_eq!(
            super::compact_tool_path(
                "/Users/krabswang/Workspaces/iPhoneqq/QQMainProject/Submodules/VAS/VASDubhe/StrongRemind/HalfScreen/QQStrongRemindHalfScreenView.m",
                home
            ),
            "~/.../StrongRemind/HalfScreen/QQStrongRemindHalfScreenView.m"
        );
        assert_eq!(
            super::compact_tool_path("/Users/krabswang/project/src/main.rs", home),
            "~/project/src/main.rs"
        );
        assert_eq!(
            super::compact_tool_path("/Users/krabswang-other/src/main.rs", home),
            "/Users/krabswang-other/src/main.rs"
        );
        assert_eq!(
            super::compact_tool_path("src/界面/文件.rs", home),
            "src/界面/文件.rs"
        );
        assert_eq!(super::compact_tool_path("README.md", None), "README.md");
        assert_eq!(super::compact_tool_path("/Users/krabswang", home), "~");
    }

    fn tool_message(
        index: usize,
        message_type: MessageType,
        name: &str,
        call_id: &str,
    ) -> IndexedMessage {
        let mut message = SessionMessage::text(message_type, "2026-09-04T12:00:00Z", "payload");
        message.tool_name = Some(name.to_owned());
        message.call_id = Some(call_id.to_owned());
        IndexedMessage { index, message }
    }

    #[test]
    fn preview_links_only_target_explicit_file_tools() {
        let input = serde_json::json!({"file_path": "src/main.rs", "offset": 12});
        assert_eq!(
            super::tool_file_target("Read", input.as_object()),
            Some((std::path::PathBuf::from("src/main.rs"), Some(12)))
        );
        assert!(super::tool_file_target("grep", input.as_object()).is_none());
        assert!(super::tool_file_target("bash", input.as_object()).is_none());
        assert!(
            super::tool_file_target("read", serde_json::json!({"path":""}).as_object()).is_none()
        );
    }

    #[test]
    fn pairs_tool_results_by_call_id_before_position() {
        let items = vec![
            tool_message(0, MessageType::ToolUse, "Read", "read-1"),
            tool_message(1, MessageType::ToolUse, "Bash", "bash-1"),
            tool_message(2, MessageType::ToolResult, "Bash", "bash-1"),
            tool_message(3, MessageType::ToolResult, "Read", "read-1"),
        ];

        let pairs = pair_tool_messages(&items);
        assert_eq!(pairs.len(), 2);
        assert_eq!(
            pairs[0].tool_result.as_ref().map(|item| item.index),
            Some(3)
        );
        assert_eq!(
            pairs[1].tool_result.as_ref().map(|item| item.index),
            Some(2)
        );
    }

    #[test]
    fn codebuddy_turns_preserve_assistant_and_tool_chronology() {
        let messages = vec![
            SessionMessage::text(MessageType::User, "2026-09-04T12:00:00Z", "question"),
            SessionMessage::text(MessageType::Assistant, "2026-09-04T12:00:01Z", "intro"),
            tool_message(2, MessageType::ToolUse, "Skill", "skill-1").message,
            tool_message(3, MessageType::ToolResult, "Skill", "skill-1").message,
            SessionMessage::text(MessageType::Assistant, "2026-09-04T12:00:02Z", "next"),
            tool_message(5, MessageType::ToolUse, "Bash", "bash-1").message,
            tool_message(6, MessageType::ToolResult, "Bash", "bash-1").message,
        ];

        let turns = build_turns(&messages, AppType::CodeBuddy);
        assert_eq!(turns.len(), 4);
        assert_eq!(turns[0].messages.len(), 2);
        assert_eq!(turns[0].messages[0].message.message_type, MessageType::User);
        assert_eq!(
            turns[0].messages[1].message.message_type,
            MessageType::Assistant
        );
        assert_eq!(turns[1].messages.len(), 2);
        assert_eq!(
            turns[1].messages[0].message.tool_name.as_deref(),
            Some("Skill")
        );
        assert_eq!(
            turns[2].messages[0].message.message_type,
            MessageType::Assistant
        );
        assert_eq!(turns[3].messages.len(), 2);
        assert_eq!(
            turns[3].messages[0].message.tool_name.as_deref(),
            Some("Bash")
        );
    }

    #[test]
    fn parallel_same_name_tools_pair_results_by_call_id() {
        let messages = vec![
            tool_message(0, MessageType::ToolUse, "Bash", "bash-a").message,
            tool_message(1, MessageType::ToolUse, "Bash", "bash-b").message,
            tool_message(2, MessageType::ToolResult, "Bash", "bash-a").message,
            tool_message(3, MessageType::ToolResult, "Bash", "bash-b").message,
        ];

        let turns = build_turns(&messages, AppType::CodeBuddy);
        assert_eq!(turns.len(), 2);
        for (turn, call_id) in turns.iter().zip(["bash-a", "bash-b"]) {
            assert_eq!(turn.messages.len(), 2);
            assert!(
                turn.messages
                    .iter()
                    .all(|message| message.message.call_id.as_deref() == Some(call_id))
            );
        }
    }

    #[test]
    fn unmatched_explicit_ids_remain_orphans_but_missing_ids_can_pair() {
        for result_name in ["Read", "Bash"] {
            let items = vec![
                tool_message(0, MessageType::ToolUse, "Read", "a"),
                tool_message(1, MessageType::ToolResult, result_name, "b"),
            ];
            let pairs = pair_tool_messages(&items);
            assert_eq!(pairs.len(), 2);
            assert!(pairs[0].tool_result.is_none());
            assert!(pairs[1].tool_use.is_none());
            let messages = items
                .into_iter()
                .map(|item| item.message)
                .collect::<Vec<_>>();
            let turns = build_turns(&messages, AppType::Claude);
            assert_eq!(turns.len(), 2);
        }
        for missing_use in [false, true] {
            let mut items = vec![
                tool_message(0, MessageType::ToolUse, "Read", "a"),
                tool_message(1, MessageType::ToolResult, "Read", "b"),
            ];
            items[usize::from(!missing_use)].message.call_id = None;
            assert_eq!(pair_tool_messages(&items).len(), 1);
            let messages = items
                .into_iter()
                .map(|item| item.message)
                .collect::<Vec<_>>();
            assert_eq!(build_turns(&messages, AppType::Claude).len(), 1);
        }
    }

    struct AssistantOrderTestView {
        owner: gpui_kit::Entity<crate::app::YesSessions>,
    }

    impl gpui_kit::Render for AssistantOrderTestView {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            cx: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            let mut intro = tool_message(0, MessageType::Assistant, "", "");
            intro.message.content = Some("First explain the plan".into());
            let mut conclusion = tool_message(3, MessageType::Assistant, "", "");
            conclusion.message.content = Some("Then explain the result".into());
            super::render_assistant_group(
                0,
                vec![
                    intro,
                    tool_message(1, MessageType::ToolUse, "Read", "read"),
                    tool_message(2, MessageType::ToolResult, "Read", "read"),
                    conclusion,
                ],
                super::ConversationOptions {
                    language: yes_core::Language::En,
                    provider: AppType::Claude,
                    is_subagent: false,
                    show_thinking: false,
                    chat_bubbles: false,
                    collapse_tool_blocks: true,
                },
                &Default::default(),
                &Default::default(),
                self.owner.downgrade(),
                cx,
            )
        }
    }

    #[gpui_kit::test]
    fn claude_renders_text_before_and_after_its_tool(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{AppContext as _, px, size};
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(800.), px(600.)), |window, cx| {
            let owner = cx.new(|cx| crate::app::YesSessions::new(window, cx));
            AssistantOrderTestView { owner }
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        let intro = visual.debug_bounds("assistant-section-0").unwrap();
        let tool = visual.debug_bounds("assistant-section-1").unwrap();
        let conclusion = visual.debug_bounds("assistant-section-3").unwrap();
        assert!(intro.bottom() <= tool.top());
        assert!(tool.bottom() <= conclusion.top());
        assert!(visual.debug_bounds("assistant-section-2").is_none());
    }

    struct SubagentSummaryTestView {
        owner: gpui_kit::Entity<crate::app::YesSessions>,
        language: yes_core::Language,
    }

    impl gpui_kit::Render for SubagentSummaryTestView {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            cx: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            use gpui_kit::base::StyledExt as _;
            use gpui_kit::{ParentElement as _, Styled as _};
            let mut call = tool_message(0, MessageType::ToolUse, "Agent", "task");
            call.message.sub_agent_session_id = Some("child".into());
            call.message.tool_input = Some(serde_json::Map::from_iter([(
                "description".into(),
                serde_json::json!(
                    "检查客户端与服务端的会话渲染实现 including_a_very_long_unbroken_identifier_that_should_not_cover_the_navigation_button"
                ),
            )]));
            gpui_kit::div().size_full().v_flex().child(
                super::render_subagent(
                    0,
                    Some(&call),
                    None,
                    super::ConversationOptions {
                        language: self.language,
                        provider: AppType::Claude,
                        is_subagent: false,
                        show_thinking: false,
                        chat_bubbles: false,
                        collapse_tool_blocks: true,
                    },
                    &Default::default(),
                    self.owner.downgrade(),
                    cx,
                )
                .unwrap(),
            )
        }
    }

    #[gpui_kit::test]
    fn subagent_summary_keeps_navigation_visible_in_narrow_panels(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{AppContext as _, px, size};
        cx.update(gpui_kit::init);
        for language in [yes_core::Language::Zh, yes_core::Language::En] {
            for width in [360., 900.] {
                let window = cx.open_window(size(px(width), px(300.)), |window, cx| {
                    let owner = cx.new(|cx| crate::app::YesSessions::new(window, cx));
                    SubagentSummaryTestView { owner, language }
                });
                let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
                let summary = visual.debug_bounds("subagent-summary").unwrap();
                let action = visual.debug_bounds("subagent-open-action").unwrap();
                assert!(summary.size.height < px(150.));
                assert!(action.left() >= summary.left());
                assert!(action.right() <= summary.right());
                assert!(action.bottom() <= summary.bottom());
            }
        }
    }

    #[test]
    fn compact_message_time_uses_the_local_timezone() {
        let timestamp = "2026-09-03T13:35:00Z";
        let expected = chrono::DateTime::parse_from_rfc3339(timestamp)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%H:%M")
            .to_string();

        assert_eq!(display_time(timestamp), expected);

        let expected_datetime = chrono::DateTime::parse_from_rfc3339(timestamp)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y/%m/%d %H:%M")
            .to_string();
        assert_eq!(display_datetime(timestamp), expected_datetime);
        assert_eq!(
            display_datetime("2026-09-03T21:35:00+08:00"),
            expected_datetime
        );
    }

    #[test]
    fn preserves_orphan_tool_results() {
        let pairs = pair_tool_messages(&[tool_message(
            4,
            MessageType::ToolResult,
            "Unknown",
            "missing",
        )]);

        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].tool_use.is_none());
        assert_eq!(
            pairs[0].tool_result.as_ref().map(|item| item.index),
            Some(4)
        );
    }

    #[test]
    fn classifies_tool_types_with_legacy_priority() {
        let cases = [
            ("mcp:filesystem", ToolType::Mcp),
            ("Skill", ToolType::Mcp),
            ("spawn_agent", ToolType::Subagent),
            ("EnterPlanMode", ToolType::Plan),
            ("apply_patch", ToolType::Filesystem),
            ("read_file", ToolType::Filesystem),
            ("web_search", ToolType::Search),
            ("exec_command", ToolType::Code),
            ("custom", ToolType::Generic),
        ];

        for (name, expected) in cases {
            assert_eq!(tool_type(name), expected, "unexpected type for {name}");
        }
    }

    #[test]
    fn formats_legacy_tool_display_names() {
        assert_eq!(tool_display_name("read"), "Read File");
        assert_eq!(tool_display_name("exec_command"), "Execute Command");
        assert_eq!(tool_display_name("EnterPlanMode"), "Enter Plan Mode");
        assert_eq!(tool_display_name("mcp:browser"), "MCP browser");
        assert_eq!(tool_display_name("custom_tool"), "Custom_tool");
    }

    #[test]
    fn derives_legacy_tool_summaries() {
        let read = json!({ "file_path": "/Users/example/src/main.rs" });
        assert_eq!(
            tool_summary("read", read.as_object()),
            Some("main.rs".into())
        );

        let grep = json!({ "pattern": "ToolCall", "path": "/repo/src" });
        assert_eq!(
            tool_summary("grep", grep.as_object()),
            Some("\"ToolCall\" in src".into())
        );

        let command = "x".repeat(51);
        let bash = json!({ "command": command });
        assert_eq!(
            tool_summary("bash", bash.as_object()),
            Some(format!("{}...", "x".repeat(50)))
        );

        let glob = json!({ "glob": "**/*.rs" });
        assert_eq!(
            tool_summary("glob", glob.as_object()),
            Some("**/*.rs".into())
        );
    }

    #[test]
    fn renders_tool_input_as_key_value_rows() {
        let input = json!({
            "enabled": true,
            "options": { "depth": 2 },
            "path": "/tmp/example.rs",
            "value": null
        });
        let rows = tool_input_rows(input.as_object(), false);

        assert!(rows.contains(&("enabled".into(), "true".into())));
        assert!(rows.contains(&("options".into(), "{\"depth\":2}".into())));
        assert!(rows.contains(&("path".into(), "/tmp/example.rs".into())));
        assert!(rows.contains(&("value".into(), "null".into())));
    }
}
