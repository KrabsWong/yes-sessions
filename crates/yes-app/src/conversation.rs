use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use gpui_kit::base::StyledExt;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName,
    button::{Button, ButtonVariants as _},
    message_scroller::{MessageScroller, MessageScrollerState},
    text::{TextView, TextViewStyle},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use yes_core::mermaid::{ContentSegment, split_mermaid_blocks};
use yes_core::{AppType, Language, MessageType, SessionDetail, SessionMessage};

use crate::{app::YesSessions, app_assets::ProviderIcon, i18n::tr, mermaid::MermaidDiagram};

#[derive(Clone, Copy)]
pub struct ConversationOptions {
    pub language: Language,
    pub provider: AppType,
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

fn tool_use_name_matches_result(tool_use: &IndexedMessage, tool_result: &IndexedMessage) -> bool {
    tool_use.message.message_type == MessageType::ToolUse
        && tool_result.message.message_type == MessageType::ToolResult
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

fn turn_has_unmatched_tool_use(turn: &ConversationTurn) -> bool {
    turn.messages.iter().any(|candidate| {
        candidate.message.message_type == MessageType::ToolUse
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
                    .filter(|turn| turn_has_unmatched_tool_use(turn))
                {
                    turn.messages.push(item);
                } else if let Some(turn) = turns
                    .iter_mut()
                    .rev()
                    .find(|turn| turn_has_unmatched_tool_use(turn))
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
                                        && pair.tool_use.as_ref().and_then(|message| {
                                            message.message.tool_name.as_deref()
                                        }) == Some(tool_name)
                                })
                            })
                        })
                        .or_else(|| pairs.iter().position(|pair| pair.tool_result.is_none()));
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

fn display_datetime(timestamp: &str) -> String {
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
) -> Vec<(String, String)> {
    input
        .into_iter()
        .flat_map(|input| input.iter())
        .map(|(key, value)| (key.clone(), format_tool_value(value)))
        .collect()
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
    let mut body = div().v_flex().gap_2().min_w_0().w_full().text_sm();
    let mut diagram_index = 0;
    for segment in split_mermaid_blocks(&content) {
        match segment {
            ContentSegment::Markdown(markdown) => {
                body = body.child(conversation_markdown(
                    (
                        ElementId::from(("message-content", item.index)),
                        diagram_index.to_string(),
                    ),
                    markdown,
                ));
            }
            ContentSegment::Mermaid(source) => {
                if let Some(diagram) = mermaid_views.get(&(item.index, diagram_index)) {
                    body = body.child(diagram.clone());
                } else {
                    body = body.child(conversation_markdown(
                        (
                            ElementId::from(("mermaid-fallback", item.index)),
                            diagram_index.to_string(),
                        ),
                        format!("```mermaid\n{source}\n```"),
                    ));
                }
                diagram_index += 1;
            }
        }
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
        .bg(cx.theme().selection)
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
    let input_rows = tool_input_rows(input);
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
            Button::new(("tool-toggle", message_index))
                .ghost()
                .w_full()
                .h(px(38.))
                .px_3()
                .bg(cx.theme().button)
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
                                .ml_auto()
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
                        .child(
                            Icon::new(if is_expanded {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .size(px(16.))
                            .text_color(cx.theme().muted_foreground),
                        )
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
                    .when(input_rows.is_empty(), |section| {
                        section.child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(tr(options.language, "message.noInput")),
                        )
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

pub(crate) fn inline_subagent_scope_base(session_id: &str) -> usize {
    10_000_000
        + session_id.bytes().fold(0usize, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(byte as usize)
        }) % 1_000_000
            * 1_000
}

fn render_inline_subagent(
    detail: &SessionDetail,
    outer_turn_index: usize,
    options: ConversationOptions,
    expanded: &HashSet<usize>,
    expanded_conversations: &HashSet<String>,
    inline_details: &HashMap<String, Arc<SessionDetail>>,
    loading_inline: &HashSet<String>,
    failed_inline: &HashSet<String>,
    mermaid_views: &HashMap<(usize, usize), Entity<MermaidDiagram>>,
    owner: WeakEntity<YesSessions>,
    cx: &App,
) -> AnyElement {
    let scope_base = inline_subagent_scope_base(&detail.session.id);
    let mut turns = build_turns(&detail.messages, detail.session.app_type);
    for turn in &mut turns {
        for item in &mut turn.messages {
            item.index = scope_base.saturating_add(item.index);
        }
    }
    let inline_options = ConversationOptions {
        provider: detail.session.app_type,
        collapse_tool_blocks: false,
        ..options
    };
    div()
        .rounded_md()
        .bg(cx.theme().background.opacity(0.72))
        .p_3()
        .v_flex()
        .gap_4()
        .children(turns.into_iter().enumerate().map(|(index, turn)| {
            render_turn(
                scope_base.saturating_add(index),
                outer_turn_index,
                turn,
                inline_options,
                expanded,
                expanded_conversations,
                inline_details,
                loading_inline,
                failed_inline,
                mermaid_views,
                owner.clone(),
                cx,
            )
        }))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_subagent(
    turn_index: usize,
    tool_use: Option<&IndexedMessage>,
    tool_result: Option<&IndexedMessage>,
    options: ConversationOptions,
    expanded: &HashSet<usize>,
    expanded_conversations: &HashSet<String>,
    inline_details: &HashMap<String, Arc<SessionDetail>>,
    loading_inline: &HashSet<String>,
    failed_inline: &HashSet<String>,
    mermaid_views: &HashMap<(usize, usize), Entity<MermaidDiagram>>,
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
        .unwrap_or_else(|| tr(options.language, "sessions.subAgentDefaultDesc").to_owned());
    let agent_type =
        subagent_string(input, "subagent_type").or_else(|| subagent_string(input, "type"));
    let model = subagent_string(input, "model")
        .or_else(|| tool_use.and_then(|item| item.message.model.clone()))
        .or_else(|| {
            tool_result.and_then(|item| {
                item.message
                    .metadata
                    .get("model")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            })
        })
        .or_else(|| tool_result.and_then(|item| item.message.model.clone()))
        .filter(|model| model != "default");
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
    let status = tool_result
        .and_then(|item| item.message.metadata.get("subtype"))
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
    let timestamp = tool_use
        .map(|item| item.message.timestamp.as_str())
        .or_else(|| tool_result.map(|item| item.message.timestamp.as_str()))
        .map(display_datetime)
        .unwrap_or_default();
    let output = tool_output_text(tool_result.map(|item| &item.message))
        .or_else(|| tool_output_text(tool_use.map(|item| &item.message)))
        .unwrap_or_default();
    let output_lines = output.lines().collect::<Vec<_>>();
    let total_output_lines = output_lines.len();
    let should_collapse_output = total_output_lines > 20;
    let message_index = item.index;
    let output_expanded = expanded.contains(&message_index);
    let display_output = if should_collapse_output && !output_expanded {
        format!("{}\n\n...", output_lines[..20].join("\n"))
    } else {
        output.clone()
    };
    let conversation_expanded = child_id
        .as_ref()
        .is_some_and(|child_id| expanded_conversations.contains(child_id));
    let inline_detail = child_id
        .as_ref()
        .and_then(|child_id| inline_details.get(child_id));
    let is_loading = child_id
        .as_ref()
        .is_some_and(|child_id| loading_inline.contains(child_id));
    let has_failed = child_id
        .as_ref()
        .is_some_and(|child_id| failed_inline.contains(child_id));
    let purple = hsla(
        271. / 360.,
        0.78,
        if cx.theme().mode.is_dark() {
            0.72
        } else {
            0.52
        },
        1.,
    );
    let purple_border = hsla(
        271. / 360.,
        0.62,
        if cx.theme().mode.is_dark() {
            0.34
        } else {
            0.82
        },
        0.75,
    );
    let purple_surface = hsla(
        271. / 360.,
        0.58,
        if cx.theme().mode.is_dark() {
            0.16
        } else {
            0.97
        },
        0.82,
    );

    Some(
        div()
            .rounded_lg()
            .border_1()
            .border_color(purple_border)
            .bg(purple_surface)
            .overflow_hidden()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(purple_border)
                    .bg(purple_surface)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(purple)
                            .child(Icon::new(IconName::Bot).size(px(16.)).text_color(purple))
                            .child(tr(options.language, "sessions.subAgent")),
                    )
                    .when_some(agent_type, |view, agent_type| {
                        view.child(
                            div()
                                .rounded_full()
                                .bg(purple.opacity(0.13))
                                .px_2()
                                .py(px(2.))
                                .text_size(px(11.))
                                .text_color(purple)
                                .child(agent_type),
                        )
                    })
                    .when_some(model, |view, model| {
                        view.child(
                            div()
                                .max_w(px(190.))
                                .truncate()
                                .rounded_full()
                                .bg(cx.theme().primary.opacity(0.10))
                                .px_2()
                                .py(px(2.))
                                .text_size(px(11.))
                                .text_color(cx.theme().primary)
                                .child(model),
                        )
                    })
                    .child(
                        div()
                            .ml_auto()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(timestamp),
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
                    ),
            )
            .child(
                div()
                    .p_3()
                    .v_flex()
                    .gap_3()
                    .child(
                        div()
                            .v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(tr(options.language, "sessions.task")),
                            )
                            .child(div().text_sm().line_clamp(3).child(description)),
                    )
                    .when(tool_result.is_some() && !output.is_empty(), |view| {
                        view.child(
                            div()
                                .border_t_1()
                                .border_color(purple_border.opacity(0.65))
                                .pt_3()
                                .v_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(tr(options.language, "sessions.output")),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .min_w_0()
                                        .rounded(px(4.))
                                        .bg(cx.theme().muted.opacity(0.72))
                                        .p_2()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(11.))
                                        .text_color(cx.theme().foreground.opacity(0.78))
                                        .whitespace_normal()
                                        .child(display_output),
                                )
                                .when(should_collapse_output, |section| {
                                    section.child(
                                        Button::new(("sub-agent-output", message_index))
                                            .text()
                                            .compact()
                                            .h(px(24.))
                                            .text_size(px(11.))
                                            .icon(if output_expanded {
                                                IconName::ChevronUp
                                            } else {
                                                IconName::Maximize
                                            })
                                            .label(if output_expanded {
                                                tr(options.language, "message.collapse").to_owned()
                                            } else {
                                                format!(
                                                    "{} ({} {})",
                                                    tr(options.language, "sessions.expandAll"),
                                                    total_output_lines,
                                                    tr(options.language, "sessions.lines")
                                                )
                                            })
                                            .on_click({
                                                let owner = owner.clone();
                                                move |_, _, cx| {
                                                    let _ = owner.update(cx, |this, cx| {
                                                        this.toggle_message(
                                                            message_index,
                                                            turn_index,
                                                            cx,
                                                        )
                                                    });
                                                }
                                            }),
                                    )
                                }),
                        )
                    })
                    .when_some(child_id.clone(), |view, child_id| {
                        let inline_child_id = child_id.clone();
                        let navigate_child_id = child_id.clone();
                        let inline_owner = owner.clone();
                        let navigate_owner = owner.clone();
                        view.child(
                            div()
                                .grid()
                                .grid_cols(2)
                                .gap_2()
                                .child(
                                    Button::new(("sub-agent-inline", message_index))
                                        .ghost()
                                        .w_full()
                                        .h(px(30.))
                                        .bg(purple.opacity(0.10))
                                        .text_size(px(11.))
                                        .text_color(purple)
                                        .icon(if conversation_expanded {
                                            IconName::ChevronUp
                                        } else {
                                            IconName::ChevronDown
                                        })
                                        .label(if conversation_expanded {
                                            tr(options.language, "sessions.collapseSubAgentSession")
                                        } else {
                                            tr(options.language, "sessions.expandSubAgentSession")
                                        })
                                        .on_click(move |_, _, cx| {
                                            let _ = inline_owner.update(cx, |this, cx| {
                                                this.toggle_inline_sub_agent(
                                                    inline_child_id.clone(),
                                                    turn_index,
                                                    cx,
                                                )
                                            });
                                        }),
                                )
                                .child(
                                    Button::new(("sub-agent-open", message_index))
                                        .ghost()
                                        .w_full()
                                        .h(px(30.))
                                        .bg(cx.theme().muted.opacity(0.72))
                                        .text_size(px(11.))
                                        .text_color(cx.theme().primary)
                                        .icon(IconName::ExternalLink)
                                        .label(tr(options.language, "sessions.viewSubAgentSession"))
                                        .on_click(move |_, _, cx| {
                                            let _ = navigate_owner.update(cx, |this, cx| {
                                                this.open_sub_agent(&navigate_child_id, cx)
                                            });
                                        }),
                                ),
                        )
                    })
                    .when(conversation_expanded, |view| {
                        view.child(
                            div()
                                .border_t_1()
                                .border_color(purple_border.opacity(0.65))
                                .pt_3()
                                .when(is_loading, |section| {
                                    section.child(
                                        div()
                                            .h(px(54.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .gap_2()
                                            .text_size(px(11.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(Icon::new(IconName::Loader).size(px(14.)))
                                            .child(tr(
                                                options.language,
                                                "sessions.loadingConversation",
                                            )),
                                    )
                                })
                                .when(has_failed, |section| {
                                    section.child(
                                        div()
                                            .h(px(54.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(11.))
                                            .text_color(cx.theme().danger)
                                            .child(tr(options.language, "sessions.error")),
                                    )
                                })
                                .when_some(inline_detail, |section, detail| {
                                    section.child(render_inline_subagent(
                                        detail,
                                        turn_index,
                                        options,
                                        expanded,
                                        expanded_conversations,
                                        inline_details,
                                        loading_inline,
                                        failed_inline,
                                        mermaid_views,
                                        owner.clone(),
                                        cx,
                                    ))
                                }),
                        )
                    }),
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
                .font_weight(FontWeight::MEDIUM)
                .child(tr(options.language, "message.user")),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(display_time(&item.message.timestamp)),
        );
    let bubble = div()
        .rounded_lg()
        .bg(cx.theme().selection)
        .p_3()
        .text_sm()
        .when(options.chat_bubbles, |view| {
            view.border_1()
                .border_color(cx.theme().primary.opacity(0.2))
        })
        .child(message_content(item, mermaid_views));
    let user_avatar = avatar(
        IconName::User,
        if options.chat_bubbles {
            cx.theme().primary_foreground
        } else {
            cx.theme().primary
        },
        if options.chat_bubbles {
            cx.theme().primary
        } else {
            cx.theme().selection
        },
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

fn render_assistant_group(
    turn_index: usize,
    items: Vec<IndexedMessage>,
    options: ConversationOptions,
    expanded: &HashSet<usize>,
    expanded_conversations: &HashSet<String>,
    inline_details: &HashMap<String, Arc<SessionDetail>>,
    loading_inline: &HashSet<String>,
    failed_inline: &HashSet<String>,
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
    let mut body = div().v_flex().gap_2().min_w_0().w_full();
    for pair in pair_tool_messages(&items) {
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
            expanded_conversations,
            inline_details,
            loading_inline,
            failed_inline,
            mermaid_views,
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
    }
    for item in items
        .iter()
        .filter(|item| item.message.message_type == MessageType::Assistant)
    {
        body = body
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
                |view| view.child(message_content(item, mermaid_views)),
            );
    }
    div()
        .w_full()
        .flex()
        .items_start()
        .gap_3()
        .child(avatar(
            ProviderIcon::from(options.provider),
            cx.theme().primary,
            cx.theme().selection,
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
    expanded_conversations: &HashSet<String>,
    inline_details: &HashMap<String, Arc<SessionDetail>>,
    loading_inline: &HashSet<String>,
    failed_inline: &HashSet<String>,
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
        .children(systems.iter().map(|item| render_system(item, options, cx)))
        .when_some(user, |view, item| {
            view.child(render_user(&item, options, mermaid_views, cx))
        })
        .when(!assistant.is_empty(), |view| {
            view.child(render_assistant_group(
                remeasure_turn_index,
                assistant,
                options,
                expanded,
                expanded_conversations,
                inline_details,
                loading_inline,
                failed_inline,
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
    expanded_conversations: HashSet<String>,
    inline_details: Arc<HashMap<String, Arc<SessionDetail>>>,
    loading_inline: HashSet<String>,
    failed_inline: HashSet<String>,
    mermaid_views: Arc<HashMap<(usize, usize), Entity<MermaidDiagram>>>,
    owner: WeakEntity<YesSessions>,
) -> MessageScroller {
    let turns = Arc::new(build_turns(&messages, options.provider));
    MessageScroller::new("conversation", state, move |index, _window, cx| {
        let Some(turn) = turns.get(index).cloned() else {
            return div().into_any_element();
        };
        render_turn(
            index,
            index,
            turn,
            options,
            &expanded,
            &expanded_conversations,
            &inline_details,
            &loading_inline,
            &failed_inline,
            &mermaid_views,
            owner.clone(),
            cx,
        )
    })
    .with_jump_button_label(tr(options.language, "sessions.jumpLatest"))
}

#[cfg(test)]
mod tests {
    use super::{
        IndexedMessage, ToolType, build_turns, display_time, pair_tool_messages, tool_display_name,
        tool_input_rows, tool_summary, tool_type,
    };
    use serde_json::json;
    use yes_core::{AppType, MessageType, SessionMessage};

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
    fn compact_message_time_uses_the_local_timezone() {
        let timestamp = "2026-09-03T13:35:00Z";
        let expected = chrono::DateTime::parse_from_rfc3339(timestamp)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%H:%M")
            .to_string();

        assert_eq!(display_time(timestamp), expected);
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
        let rows = tool_input_rows(input.as_object());

        assert!(rows.contains(&("enabled".into(), "true".into())));
        assert!(rows.contains(&("options".into(), "{\"depth\":2}".into())));
        assert!(rows.contains(&("path".into(), "/tmp/example.rs".into())));
        assert!(rows.contains(&("value".into(), "null".into())));
    }
}
