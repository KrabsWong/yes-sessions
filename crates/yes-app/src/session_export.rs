use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use chrono::{Local, TimeZone as _};
use markdown::Options as MarkdownOptions;
use yes_core::{
    Language, Session, SessionDetail, SessionProvider,
    model::{AttachmentSource, MessageType},
};

use crate::{
    conversation::{
        export_tool_output_text, export_tool_pairs, export_tool_summary, export_tool_title,
        export_turn_indices,
    },
    i18n::tr,
};

const MAX_CHILDREN: usize = 200;

#[derive(Clone, Copy)]
pub struct ExportOptions {
    pub language: Language,
    pub chat_bubbles: bool,
    pub show_thinking: bool,
    pub collapse_tools: bool,
}

pub struct ExportBundle {
    pub details: Vec<Arc<SessionDetail>>,
    pub missing_children: Vec<String>,
}

struct ExportFile {
    relative_path: PathBuf,
    html: String,
}

pub fn collect_bundle(
    root: Arc<SessionDetail>,
    sessions: Arc<Vec<Session>>,
    provider: Arc<dyn SessionProvider>,
) -> Result<ExportBundle> {
    let mut details = vec![root.clone()];
    let mut visited = HashSet::new();
    visited.insert(root.session.id.clone());
    if let Some(uuid) = root.session.uuid.as_deref() {
        visited.insert(uuid.to_owned());
    }

    let mut pending = Vec::new();
    queue_children(&mut pending, &root, &sessions);
    let mut missing_children = Vec::new();

    while let Some(id) = pending.pop() {
        if visited.contains(&id) {
            continue;
        }
        if details.len() >= MAX_CHILDREN {
            missing_children.push(id);
            continue;
        }

        let loaded = find_session(&sessions, &id)
            .cloned()
            .map(|session| provider.session_detail_from_summary(&session))
            .unwrap_or_else(|| provider.session_detail(&id));
        let detail = match loaded {
            Ok(Some(detail)) => Arc::new(detail),
            Ok(None) => {
                missing_children.push(id);
                continue;
            }
            Err(_) => {
                missing_children.push(id);
                continue;
            }
        };

        visited.insert(detail.session.id.clone());
        if let Some(uuid) = detail.session.uuid.as_deref() {
            visited.insert(uuid.to_owned());
        }
        queue_children(&mut pending, &detail, &sessions);
        details.push(detail);
    }

    Ok(ExportBundle {
        details,
        missing_children,
    })
}

fn find_session<'a>(sessions: &'a [Session], id: &str) -> Option<&'a Session> {
    sessions
        .iter()
        .find(|session| session.id == id || session.uuid.as_deref() == Some(id))
}

fn queue_children(pending: &mut Vec<String>, detail: &SessionDetail, sessions: &[Session]) {
    for message in &detail.messages {
        if let Some(child) = message.sub_agent_session_id.as_deref() {
            pending.push(child.to_owned());
        }
    }
    for session in sessions {
        let Some(parent) = session.parent_session_id.as_deref() else {
            continue;
        };
        if parent == detail.session.id
            || detail
                .session
                .uuid
                .as_deref()
                .is_some_and(|uuid| parent == uuid)
        {
            pending.push(session.id.clone());
        }
    }
}

pub fn suggested_file_name(detail: &SessionDetail) -> String {
    let title = if detail.session.first_message.trim().is_empty() {
        detail.session.file_name.as_str()
    } else {
        detail.session.first_message.as_str()
    };
    let safe = safe_component(title, 100);
    format!("{}.html", if safe.is_empty() { "session" } else { &safe })
}

pub fn normalize_destination(path: &Path) -> PathBuf {
    let mut path = path.to_path_buf();
    let is_html = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("html"))
        || path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("htm"));
    if !is_html {
        path.set_extension("html");
    }
    path
}

pub fn write_bundle(
    destination: &Path,
    bundle: &ExportBundle,
    options: ExportOptions,
) -> Result<()> {
    let file_name = safe_component(
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("session.html"),
        180,
    );
    let file_name = if file_name.is_empty() {
        "session.html".to_owned()
    } else {
        file_name
    };
    let stem = Path::new(&file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("session");
    let subdir = format!("{}_subagents", safe_component(stem, 120));
    let files = render_bundle(bundle, options, &file_name, &subdir);
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));

    for file in files {
        let path = parent.join(file.relative_path);
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory)?;
        }
        fs::write(path, file.html)?;
    }
    Ok(())
}

fn render_bundle(
    bundle: &ExportBundle,
    options: ExportOptions,
    main_file_name: &str,
    subdir: &str,
) -> Vec<ExportFile> {
    let mut index_by_id = HashMap::new();
    for (index, detail) in bundle.details.iter().enumerate() {
        index_by_id.insert(detail.session.id.clone(), index);
        if let Some(uuid) = detail.session.uuid.as_deref() {
            index_by_id.insert(uuid.to_owned(), index);
        }
    }

    bundle
        .details
        .iter()
        .enumerate()
        .map(|(index, detail)| ExportFile {
            relative_path: if index == 0 {
                PathBuf::from(main_file_name)
            } else {
                PathBuf::from(subdir).join(format!("agent-{index}.html"))
            },
            html: render_page(
                detail,
                index,
                &bundle.details,
                options,
                main_file_name,
                subdir,
                &index_by_id,
                &bundle.missing_children,
            ),
        })
        .collect()
}

fn render_page(
    detail: &SessionDetail,
    index: usize,
    details: &[Arc<SessionDetail>],
    options: ExportOptions,
    main_file_name: &str,
    subdir: &str,
    index_by_id: &HashMap<String, usize>,
    missing_children: &[String],
) -> String {
    let language = options.language;
    let mut html = String::new();
    let title = page_title(detail);
    let counts = message_counts(detail);
    let _ = writeln!(
        html,
        "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"light dark\">\
         <title>{}</title><style>{}</style></head><body>",
        if language == Language::Zh {
            "zh-CN"
        } else {
            "en"
        },
        escape_text(&title),
        styles(),
    );

    render_filter_inputs(&mut html);

    let _ = writeln!(html, "<header><div class=\"wrap\">");
    let _ = writeln!(html, "<h1>{}</h1>", escape_text(&title));
    let kind = if detail.session.kind == yes_core::model::SessionKind::Subagent {
        tr(language, "sessions.subAgent")
    } else {
        tr(language, "message.mainAgent")
    };
    let _ = writeln!(
        html,
        "<div class=\"meta\"><span>{}</span><span aria-hidden=\"true\">·</span><span>{}</span><span aria-hidden=\"true\">·</span><span>{}</span><span aria-hidden=\"true\">·</span><span>{}</span>",
        escape_text(kind),
        detail.session.app_type.display_name(),
        escape_text(&format_timestamp(detail.session.updated_at)),
        escape_text(&tr(language, "export.messageCount").replace("{count}", &counts.0.to_string())),
    );
    if let Some(directory) = detail.session.directory.as_deref() {
        let _ = writeln!(
            html,
            "<span class=\"path\">{}</span>",
            escape_text(&directory.display().to_string())
        );
    }
    let _ = writeln!(html, "</div>");
    if index > 0 {
        let back = relative_href(index, 0, main_file_name, subdir);
        let _ = writeln!(
            html,
            "<a class=\"back\" href=\"{}\">← {}</a>",
            escape_attr(&back),
            escape_text(&tr(language, "export.backToMain"))
        );
    }
    let _ = writeln!(html, "</div></header>");

    render_filter_bar(&mut html, language, counts);

    let _ = writeln!(html, "<main class=\"wrap\">");
    if index == 0 && details.len() > 1 {
        let _ = writeln!(
            html,
            "<section class=\"subagents\"><h2>{}</h2><ol>",
            escape_text(&tr(language, "export.subagents"))
        );
        for child_index in 1..details.len() {
            let href = relative_href(index, child_index, main_file_name, subdir);
            let child_title = page_title(&details[child_index]);
            let _ = writeln!(
                html,
                "<li><a href=\"{}\">{}</a></li>",
                escape_attr(&href),
                escape_text(&child_title)
            );
        }
        let _ = writeln!(html, "</ol></section>");
    }

    if index == 0 && !missing_children.is_empty() {
        let _ = writeln!(
            html,
            "<p class=\"notice\">{}: {}</p>",
            escape_text(&tr(language, "export.missing")),
            escape_text(&missing_children.join(", "))
        );
    }

    let _ = writeln!(
        html,
        "<section class=\"transcript\" aria-label=\"{}\">",
        escape_attr(&tr(language, "export.title"))
    );
    render_app_like_turns(
        &mut html,
        detail,
        options,
        index,
        main_file_name,
        subdir,
        index_by_id,
    );
    let _ = writeln!(html, "</section>");

    let _ = writeln!(
        html,
        "<footer>{} {}</footer></main></body></html>",
        escape_text(&tr(language, "export.generatedAt")),
        escape_text(&Local::now().format("%Y-%m-%d %H:%M:%S %z").to_string())
    );
    html
}

fn message_counts(detail: &SessionDetail) -> (usize, usize, usize) {
    let user = detail
        .messages
        .iter()
        .filter(|message| message.message_type == MessageType::User)
        .count();
    let assistant = detail
        .messages
        .iter()
        .filter(|message| message.message_type == MessageType::Assistant)
        .count();
    (detail.messages.len(), user, assistant)
}

fn render_filter_inputs(html: &mut String) {
    for (id, checked) in [("all", true), ("user", false), ("ai", false)] {
        let _ = writeln!(
            html,
            "<input class=\"filter-input\" type=\"radio\" name=\"export-filter\" id=\"export-filter-{id}\"{}>",
            if checked { " checked" } else { "" },
        );
    }
}

fn render_filter_bar(html: &mut String, language: Language, counts: (usize, usize, usize)) {
    let _ = writeln!(
        html,
        "<div class=\"filter-bar\"><div class=\"wrap filter-row\"><span class=\"filter-label\">{}</span>",
        escape_text(&tr(language, "export.filterLabel"))
    );
    for (id, label, count) in [
        ("all", tr(language, "export.filterAll"), counts.0),
        ("user", tr(language, "export.filterUser"), counts.1),
        ("ai", tr(language, "export.filterAi"), counts.2),
    ] {
        let _ = writeln!(
            html,
            "<label class=\"filter-option\" for=\"export-filter-{id}\">{} <span>{count}</span></label>",
            escape_text(label)
        );
    }
    let _ = writeln!(html, "</div></div>");
}

fn render_app_like_turns(
    html: &mut String,
    detail: &SessionDetail,
    options: ExportOptions,
    page_index: usize,
    main_file_name: &str,
    subdir: &str,
    index_by_id: &HashMap<String, usize>,
) {
    let language = options.language;
    let turns = export_turn_indices(&detail.messages, detail.session.app_type);
    for indices in turns {
        let systems = indices
            .iter()
            .copied()
            .filter(|&index| detail.messages[index].message_type == MessageType::System)
            .collect::<Vec<_>>();
        let user = indices
            .iter()
            .copied()
            .find(|&index| detail.messages[index].message_type == MessageType::User);
        let reply = indices
            .iter()
            .copied()
            .filter(|&index| {
                matches!(
                    detail.messages[index].message_type,
                    MessageType::Assistant | MessageType::ToolUse | MessageType::ToolResult
                )
            })
            .collect::<Vec<_>>();

        let _ = writeln!(html, "<section class=\"conversation-turn\">");
        for index in systems {
            render_system_card(html, &detail.messages[index], language);
        }
        if let Some(index) = user {
            render_user_row(html, &detail.messages[index], options);
        }
        if !reply.is_empty() {
            render_assistant_group(
                html,
                detail,
                &reply,
                options,
                page_index,
                main_file_name,
                subdir,
                index_by_id,
            );
        }
        let _ = writeln!(html, "</section>");
    }
}

fn render_system_card(
    html: &mut String,
    message: &yes_core::model::SessionMessage,
    language: Language,
) {
    let _ = writeln!(
        html,
        "<div class=\"system-card\"><strong>{}</strong><span>{}</span></div>",
        escape_text(&tr(language, "message.system")),
        escape_text(&message_time(&message.timestamp))
    );
    if let Some(content) = message.content.as_deref() {
        let _ = writeln!(
            html,
            "<div class=\"system-content\">{}</div>",
            markdown_html(content, language)
        );
    }
}

fn render_user_row(
    html: &mut String,
    message: &yes_core::model::SessionMessage,
    options: ExportOptions,
) {
    let language = options.language;
    let avatar = if language == Language::Zh { "我" } else { "U" };
    let _ = writeln!(
        html,
        "<div class=\"user-row{}\">",
        if options.chat_bubbles { " bubble" } else { "" }
    );
    let _ = writeln!(
        html,
        "<span class=\"avatar user-avatar\" aria-hidden=\"true\">{}</span>",
        escape_text(avatar)
    );
    let _ = writeln!(html, "<div class=\"message-main\">");
    let _ = writeln!(
        html,
        "<div class=\"message-head\"><strong>{}</strong><time>{}</time></div>",
        escape_text(&tr(language, "message.user")),
        escape_text(&message_time(&message.timestamp))
    );
    let _ = writeln!(html, "<div class=\"user-bubble\">");
    if let Some(content) = message.content.as_deref() {
        let _ = writeln!(html, "{}", markdown_html(content, language));
    }
    render_attachments_inline(html, message, language);
    if let Some(context) = message
        .metadata
        .get("user_context")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
    {
        render_details(
            html,
            "context",
            tr(language, "message.userContext"),
            &format!("<pre>{}</pre>", escape_text(context)),
        );
    }
    if !message.metadata.is_empty() {
        render_details(
            html,
            "metadata",
            tr(language, "export.metadata"),
            &format!(
                "<pre>{}</pre>",
                escape_text(&pretty_json(&message.metadata))
            ),
        );
    }
    let _ = writeln!(html, "</div></div></div>");
}

#[allow(clippy::too_many_arguments)]
fn render_assistant_group(
    html: &mut String,
    detail: &SessionDetail,
    indices: &[usize],
    options: ExportOptions,
    page_index: usize,
    main_file_name: &str,
    subdir: &str,
    index_by_id: &HashMap<String, usize>,
) {
    let language = options.language;
    let messages = indices
        .iter()
        .map(|&index| &detail.messages[index])
        .collect::<Vec<_>>();
    let model = messages
        .iter()
        .find_map(|message| message.model.as_deref())
        .filter(|model| !model.is_empty())
        .unwrap_or_default();
    let timestamp = messages
        .first()
        .map(|message| message_time(&message.timestamp))
        .unwrap_or_default();
    let avatar = detail
        .session
        .app_type
        .display_name()
        .chars()
        .next()
        .unwrap_or('A')
        .to_string();

    let _ = writeln!(html, "<div class=\"reply-row\">");
    let _ = writeln!(
        html,
        "<span class=\"avatar assistant-avatar\" aria-hidden=\"true\">{}</span>",
        escape_text(&avatar)
    );
    let _ = writeln!(html, "<div class=\"reply-main\">");
    let _ = writeln!(
        html,
        "<div class=\"reply-head\"><strong>{}</strong>{}<time>{}</time></div>",
        escape_text(detail.session.app_type.display_name()),
        if model.is_empty() {
            String::new()
        } else {
            format!("<span class=\"model-badge\">{}</span>", escape_text(model))
        },
        escape_text(&timestamp)
    );

    let pairs = export_tool_pairs(&detail.messages, indices);
    let mut pair_by_first = HashMap::new();
    for pair in &pairs {
        let first = pair.0.or(pair.1);
        if let Some(first) = first {
            pair_by_first.insert(first, *pair);
        }
    }
    let mut rendered_pairs = HashSet::new();

    let _ = writeln!(html, "<div class=\"reply-body\">");
    for &index in indices {
        if let Some(&(tool_use, tool_result)) = pair_by_first.get(&index) {
            if rendered_pairs.insert(index) {
                render_tool_pair(
                    html,
                    detail,
                    tool_use,
                    tool_result,
                    options,
                    page_index,
                    main_file_name,
                    subdir,
                    index_by_id,
                );
            }
            continue;
        }
        let message = &detail.messages[index];
        if message.message_type != MessageType::Assistant {
            continue;
        }
        if options.show_thinking {
            if let Some(reasoning) = message
                .reasoning_content
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                render_reasoning(html, reasoning, language);
            }
        }
        if let Some(content) = message.content.as_deref().filter(|value| !value.is_empty()) {
            let _ = writeln!(
                html,
                "<div class=\"assistant-text markdown-body\">{}</div>",
                markdown_html(content, language)
            );
        }
        render_attachments_inline(html, message, language);
        if !message.metadata.is_empty() {
            render_details(
                html,
                "metadata",
                tr(language, "export.metadata"),
                &format!(
                    "<pre>{}</pre>",
                    escape_text(&pretty_json(&message.metadata))
                ),
            );
        }
    }
    let _ = writeln!(html, "</div></div></div>");
}

#[allow(clippy::too_many_arguments)]
fn render_tool_pair(
    html: &mut String,
    detail: &SessionDetail,
    tool_use: Option<usize>,
    tool_result: Option<usize>,
    options: ExportOptions,
    page_index: usize,
    main_file_name: &str,
    subdir: &str,
    index_by_id: &HashMap<String, usize>,
) {
    let language = options.language;
    let use_message = tool_use.map(|index| &detail.messages[index]);
    let result_message = tool_result.map(|index| &detail.messages[index]);
    let item = use_message.or(result_message);
    let Some(item) = item else { return };

    let tool_name = use_message
        .and_then(|message| message.tool_name.as_deref())
        .or_else(|| result_message.and_then(|message| message.tool_name.as_deref()))
        .unwrap_or("tool");
    let child_id = result_message
        .and_then(|message| message.sub_agent_session_id.as_deref())
        .or_else(|| {
            result_message.and_then(|message| {
                message
                    .metadata
                    .get("childSessionId")
                    .and_then(|value| value.as_str())
            })
        })
        .or_else(|| use_message.and_then(|message| message.sub_agent_session_id.as_deref()));
    let looks_like_subagent = child_id.is_some()
        || use_message
            .and_then(|message| message.tool_name.as_deref())
            .is_some_and(is_subagent_tool)
        || result_message
            .and_then(|message| message.tool_name.as_deref())
            .is_some_and(is_subagent_tool);

    if looks_like_subagent {
        render_subagent_card(
            html,
            use_message,
            result_message,
            child_id,
            language,
            page_index,
            main_file_name,
            subdir,
            index_by_id,
        );
        return;
    }

    let title = export_tool_title(tool_name);
    let summary = export_tool_summary(
        tool_name,
        use_message.and_then(|message| message.tool_input.as_ref()),
    )
    .or_else(|| {
        use_message
            .and_then(|message| message.content.as_deref())
            .map(|content| content.chars().take(80).collect())
    })
    .unwrap_or_default();
    let time = message_time(&item.timestamp);
    let _ = writeln!(
        html,
        "<details class=\"tool-card\"{}>",
        if options.collapse_tools { "" } else { " open" }
    );
    let _ = writeln!(
        html,
        "<summary><span class=\"tool-name\">{}</span>{}<time>{}</time></summary>",
        escape_text(&title),
        if summary.is_empty() {
            String::new()
        } else {
            format!(
                "<span class=\"tool-summary\">{}</span>",
                escape_text(&summary)
            )
        },
        escape_text(&time)
    );
    let _ = writeln!(html, "<div class=\"tool-body\">");
    if let Some(input) = use_message.and_then(|message| message.tool_input.as_ref()) {
        let _ = writeln!(
            html,
            "<h4>{}</h4><pre>{}</pre>",
            escape_text(&tr(language, "message.input")),
            escape_text(&pretty_json(input))
        );
    }
    let output = result_message
        .map(export_tool_output_text)
        .or_else(|| use_message.map(export_tool_output_text))
        .flatten();
    if let Some(output) = output {
        let _ = writeln!(
            html,
            "<h4>{}</h4><pre>{}</pre>",
            escape_text(&tr(language, "message.output")),
            escape_text(&output)
        );
    }
    let _ = writeln!(html, "</div></details>");
}

#[allow(clippy::too_many_arguments)]
fn render_subagent_card(
    html: &mut String,
    use_message: Option<&yes_core::model::SessionMessage>,
    result_message: Option<&yes_core::model::SessionMessage>,
    child_id: Option<&str>,
    language: Language,
    page_index: usize,
    main_file_name: &str,
    subdir: &str,
    index_by_id: &HashMap<String, usize>,
) {
    let description = use_message
        .and_then(|message| message.tool_input.as_ref())
        .and_then(|input| {
            ["description", "task", "prompt", "message", "task_name"]
                .into_iter()
                .find_map(|key| input.get(key).and_then(|value| value.as_str()))
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| tr(language, "sessions.subAgentDefaultDesc"));
    let status = if result_message.is_some() {
        tr(language, "sessions.completed")
    } else {
        tr(language, "sessions.running")
    };
    let _ = writeln!(html, "<div class=\"subagent-card\">");
    let _ = writeln!(
        html,
        "<div class=\"subagent-head\"><strong>{}</strong><span class=\"status\">{}</span></div>",
        escape_text(&tr(language, "sessions.subAgent")),
        escape_text(status)
    );
    let _ = writeln!(
        html,
        "<p class=\"subagent-desc\">{}</p>",
        escape_text(&description.chars().take(200).collect::<String>())
    );
    if let Some(child_id) = child_id {
        if let Some(&child_index) = index_by_id.get(child_id) {
            if child_index != page_index {
                let href = relative_href(page_index, child_index, main_file_name, subdir);
                let _ = writeln!(
                    html,
                    "<a class=\"subagent-link\" href=\"{}\">{}</a>",
                    escape_attr(&href),
                    escape_text(&tr(language, "sessions.viewSubAgentSession"))
                );
            }
        }
    }
    let _ = writeln!(html, "</div>");
}

fn render_reasoning(html: &mut String, content: &str, language: Language) {
    let _ = writeln!(
        html,
        "<details class=\"reasoning\"><summary><span class=\"reasoning-mark\">*</span>{}<span class=\"hint\">{}</span></summary>",
        escape_text(&tr(language, "message.thinking")),
        escape_text(&tr(language, "message.expand"))
    );
    let _ = writeln!(
        html,
        "<div class=\"reasoning-content markdown-body\">{}</div></details>",
        markdown_html(content, language)
    );
}

fn render_attachments_inline(
    html: &mut String,
    message: &yes_core::model::SessionMessage,
    language: Language,
) {
    if message.attachments.is_empty() {
        return;
    }
    let _ = writeln!(html, "<ul class=\"attachment-list\">");
    for attachment in &message.attachments {
        let source = match &attachment.source {
            AttachmentSource::LocalPath(_) => "local",
            AttachmentSource::DataUrl(_) => "data",
            AttachmentSource::RemoteUrl(_) => tr(language, "attachment.remote"),
        };
        let _ = writeln!(
            html,
            "<li><span>{}</span><small>{}</small></li>",
            escape_text(&attachment.name),
            escape_text(source)
        );
    }
    let _ = writeln!(html, "</ul>");
}

fn markdown_html(source: &str, language: Language) -> String {
    let mut options = MarkdownOptions::gfm();
    options.compile.allow_dangerous_html = false;
    options.compile.allow_dangerous_protocol = false;
    let html = markdown::to_html_with_options(source, &options)
        .unwrap_or_else(|_| format!("<p>{}</p>", escape_text(source)));
    neutralize_remote_images(&html, language)
}

fn neutralize_remote_images(html: &str, language: Language) -> String {
    let mut output = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<img") {
        output.push_str(&rest[..start]);
        let tag_end = rest[start..]
            .find('>')
            .map(|offset| start + offset + 1)
            .unwrap_or(rest.len());
        let tag = &rest[start..tag_end];
        if tag.contains("src=\"http") || tag.contains("src='http") || tag.contains("src=http") {
            let _ = write!(
                output,
                "<span class=\"remote-image\">[{}]</span>",
                escape_text(&tr(language, "export.remoteImage"))
            );
        } else {
            output.push_str(tag);
        }
        rest = &rest[tag_end..];
    }
    output.push_str(rest);
    output
}

fn is_subagent_tool(tool_name: &str) -> bool {
    let name = tool_name.to_ascii_lowercase();
    name == "task" || name.contains("agent") || name.contains("spawn") || name.contains("delegate")
}

fn message_time(timestamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .map(|value| {
            value
                .with_timezone(&chrono::Local)
                .format("%Y/%m/%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| timestamp.to_owned())
}

fn render_details(html: &mut String, class: &str, summary: &str, body: &str) {
    let _ = writeln!(
        html,
        "<details class=\"{class}\"><summary>{}</summary>{}</details>",
        escape_text(summary),
        body
    );
}

fn page_title(detail: &SessionDetail) -> String {
    if detail.session.first_message.trim().is_empty() {
        detail.session.file_name.clone()
    } else {
        detail.session.first_message.clone()
    }
}

fn relative_href(from: usize, to: usize, main_file_name: &str, subdir: &str) -> String {
    if from == 0 {
        if to == 0 {
            href_segment(main_file_name)
        } else {
            format!("{}/agent-{to}.html", href_segment(subdir))
        }
    } else if to == 0 {
        format!("../{}", href_segment(main_file_name))
    } else {
        format!("agent-{to}.html")
    }
}

fn href_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(*byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn format_timestamp(millis: i64) -> String {
    Local
        .timestamp_millis_opt(millis)
        .single()
        .map(|value| value.format("%Y-%m-%d %H:%M:%S %z").to_string())
        .unwrap_or_else(|| millis.to_string())
}

fn pretty_json(value: &serde_json::Map<String, serde_json::Value>) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_owned())
}

fn safe_component(name: &str, limit: usize) -> String {
    let component = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("session")
        .chars()
        .take(limit)
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '_' | '-' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    component.trim_matches('.').to_owned()
}

fn escape_text(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
    output
}

fn escape_attr(input: &str) -> String {
    escape_text(input)
}

fn styles() -> &'static str {
    r#"
:root{color-scheme:light dark;--bg:#ffffff;--panel:#f8fafc;--fg:#0f172a;--muted:#64748b;--border:#e2e8f0;--accent:#1e293b;--accent-soft:#dce5f2;--accent-soft-fg:#526986;--tool-bg:#f1f4f9;--tool-border:#d6dfed;--warning-bg:rgba(245,158,11,.08);--warning-border:rgba(245,158,11,.35);--warning:#b45309;--code-bg:#f1f5f9}
@media(prefers-color-scheme:dark){:root{--bg:#0b1220;--panel:#111827;--fg:#f1f5f9;--muted:#9aa7bd;--border:#253041;--accent:#f1f5f9;--accent-soft:#344763;--accent-soft-fg:#b8c9e4;--tool-bg:#202a3a;--tool-border:#3b4960;--warning-bg:rgba(245,158,11,.10);--warning-border:rgba(245,158,11,.35);--warning:#f5b74a;--code-bg:#151d29}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.6 -apple-system,BlinkMacSystemFont,"Segoe UI",system-ui,sans-serif}
a{color:var(--accent-soft-fg);text-decoration-thickness:1px;text-underline-offset:2px}a:focus-visible,summary:focus-visible,label:focus-visible{outline:2px solid var(--accent-soft-fg);outline-offset:2px}
.wrap{max-width:900px;margin:0 auto;padding:0 16px}header{background:var(--bg);border-bottom:1px solid var(--border);padding:20px 0 16px}h1{font-size:16px;line-height:1.4;margin:0 0 6px;font-weight:600;overflow-wrap:anywhere}.meta{display:flex;flex-wrap:wrap;gap:4px 8px;color:var(--muted);font-size:12px}.meta .path{width:100%;overflow-wrap:anywhere}.back{display:inline-block;margin-top:10px;font-size:12px}
.filter-bar{position:sticky;top:0;z-index:10;background:var(--bg);border-bottom:1px solid var(--border);padding:8px 0}.filter-row{display:flex;align-items:center;gap:6px;flex-wrap:wrap}.filter-label{font-size:12px;color:var(--muted);margin-right:2px}
.filter-input{position:absolute;width:1px;height:1px;opacity:0;pointer-events:none}.filter-option{display:inline-flex;align-items:center;gap:5px;padding:3px 8px;border:1px solid var(--border);border-radius:4px;background:var(--bg);color:var(--fg);cursor:pointer;font-size:12px;line-height:1.3}.filter-option span{color:var(--muted);font-variant-numeric:tabular-nums}#export-filter-all:checked~.filter-bar label[for="export-filter-all"],#export-filter-user:checked~.filter-bar label[for="export-filter-user"],#export-filter-ai:checked~.filter-bar label[for="export-filter-ai"]{background:var(--accent);border-color:var(--accent);color:var(--bg)}#export-filter-all:checked~.filter-bar label[for="export-filter-all"] span,#export-filter-user:checked~.filter-bar label[for="export-filter-user"] span,#export-filter-ai:checked~.filter-bar label[for="export-filter-ai"] span{color:inherit}#export-filter-all:focus-visible~.filter-bar label[for="export-filter-all"],#export-filter-user:focus-visible~.filter-bar label[for="export-filter-user"],#export-filter-ai:focus-visible~.filter-bar label[for="export-filter-ai"]{outline:2px solid var(--accent-soft-fg);outline-offset:2px}
#export-filter-user:checked~main .reply-row,#export-filter-user:checked~main .system-card,#export-filter-user:checked~main .system-content,#export-filter-ai:checked~main .user-row,#export-filter-ai:checked~main .system-card,#export-filter-ai:checked~main .system-content,#export-filter-ai:checked~main .tool-card,#export-filter-ai:checked~main .subagent-card,#export-filter-ai:checked~main .reasoning{display:none}
main{padding:16px 0 56px}.subagents{margin-bottom:24px}.subagents h2{font-size:13px;line-height:1.3;margin:0 0 8px}.subagents ol{margin:0;padding-left:20px}.subagents li{margin:4px 0}
.notice{margin:0 0 20px;padding:8px 0;border-top:1px solid var(--border);border-bottom:1px solid var(--border);color:var(--muted);font-size:12px}
.transcript{display:block}.conversation-turn{display:flex;flex-direction:column;gap:16px;margin-bottom:24px}
.avatar{width:32px;height:32px;flex:none;border-radius:50%;display:inline-flex;align-items:center;justify-content:center;font-size:12px;font-weight:600}.user-avatar{background:var(--accent-soft);color:var(--accent-soft-fg)}.assistant-avatar{background:var(--panel);color:var(--fg);border:1px solid var(--border)}
.user-row,.reply-row{display:flex;align-items:flex-start;gap:12px;min-width:0}.user-row.bubble{justify-content:flex-end}.user-row.bubble .user-avatar{order:2}.user-row.bubble .message-main{order:1;align-items:flex-end}.message-main,.reply-main{flex:1;min-width:0;display:flex;flex-direction:column;gap:4px}.message-head,.reply-head{display:flex;align-items:center;gap:8px;font-size:12px;color:var(--muted)}.message-head strong,.reply-head strong{font-weight:600;color:var(--fg)}time{font-size:11px;color:var(--muted)}.model-badge{padding:1px 5px;border-radius:4px;background:var(--panel);color:var(--muted);font-size:11px}
.user-bubble{background:var(--accent-soft);color:var(--fg);border-radius:8px;padding:12px;max-width:85%;overflow-wrap:anywhere}.user-row.bubble .user-bubble{max-width:100%}.reply-body{display:flex;flex-direction:column;gap:12px;min-width:0}.assistant-text,.reasoning-content{font-size:15px;overflow-wrap:anywhere}
.system-card{display:flex;align-items:center;gap:8px;background:var(--panel);border:1px solid var(--border);border-radius:8px;padding:6px 12px;font-size:13px}.system-content{padding:0 12px;color:var(--muted);font-size:13px}
.tool-card,.reasoning{border:1px solid var(--border);border-radius:8px;background:var(--panel);overflow:hidden}.tool-card{border-color:var(--tool-border);background:var(--tool-bg)}.tool-card summary,.reasoning summary{display:flex;align-items:center;gap:8px;cursor:pointer;list-style:none;padding:9px 12px;font-size:12px}.tool-card summary::-webkit-details-marker,.reasoning summary::-webkit-details-marker{display:none}.tool-card summary::before,.reasoning summary::before{content:"+";font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;color:var(--accent-soft-fg)}.tool-card[open]>summary::before,.reasoning[open]>summary::before{content:"−"}.tool-name{font-weight:600;color:var(--fg)}.tool-summary{color:var(--muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.tool-card time{margin-left:auto;flex:none}.tool-body{padding:0 12px 12px}.tool-body h4{font-size:11px;margin:10px 0 6px;color:var(--muted);font-weight:600}pre{margin:0;padding:10px 12px;background:var(--code-bg);border-radius:6px;white-space:pre-wrap;overflow-wrap:anywhere;font:12px/1.5 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}
.reasoning{border-color:var(--warning-border);background:var(--warning-bg)}.reasoning summary{color:var(--warning)}.reasoning-mark{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}.reasoning .hint{margin-left:auto;color:var(--muted);font-size:11px}.reasoning-content{padding:0 12px 12px;color:var(--muted)}
.subagent-card{border:1px solid var(--tool-border);border-radius:8px;background:var(--tool-bg);padding:12px}.subagent-head{display:flex;align-items:center;gap:8px;font-size:13px}.subagent-head .status{padding:1px 6px;border-radius:999px;background:var(--bg);color:var(--muted);font-size:10px}.subagent-desc{margin:6px 0 0;color:var(--muted);font-size:13px;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden}.subagent-link{display:inline-flex;align-items:center;margin-top:8px;padding:4px 8px;border:1px solid var(--border);border-radius:6px;background:var(--bg);color:var(--fg);text-decoration:none;font-size:12px}
.attachment-list{display:flex;flex-wrap:wrap;gap:6px;list-style:none;margin:8px 0 0;padding:0}.attachment-list li{display:flex;gap:6px;align-items:center;border:1px solid var(--border);border-radius:4px;padding:2px 6px;font-size:12px}.attachment-list small{color:var(--muted)}
.markdown-body>*:first-child{margin-top:0}.markdown-body>*:last-child{margin-bottom:0}.markdown-body p{margin:8px 0}.markdown-body h1,.markdown-body h2,.markdown-body h3,.markdown-body h4{font-size:15px;line-height:1.35;margin:14px 0 6px}.markdown-body ul,.markdown-body ol{margin:8px 0;padding-left:22px}.markdown-body li{margin:2px 0}.markdown-body blockquote{margin:8px 0;padding-left:12px;border-left:1px solid var(--border);color:var(--muted)}.markdown-body code{background:var(--code-bg);border-radius:4px;padding:1px 4px;font:12px/1.5 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}.markdown-body pre{padding:10px 12px}.markdown-body pre code{background:transparent;padding:0}.markdown-body table{border-collapse:collapse;margin:8px 0;font-size:13px}.markdown-body th,.markdown-body td{border:1px solid var(--border);padding:4px 8px;text-align:left}
.remote-image{color:var(--muted);font-size:12px}
footer{margin-top:36px;padding-top:12px;border-top:1px solid var(--border);color:var(--muted);font-size:12px}
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use yes_core::{
        AppType,
        model::{MessageType, Session, SessionDetail, SessionKind, SessionMessage, ToolOutput},
    };

    fn session(id: &str, parent: Option<&str>) -> Session {
        Session {
            id: id.to_owned(),
            app_type: AppType::Claude,
            file_name: format!("{id}.jsonl"),
            file_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_060_000,
            message_count: 1,
            first_message: format!("title-{id}"),
            last_message: String::new(),
            directory: None,
            uuid: None,
            kind: if parent.is_some() {
                SessionKind::Subagent
            } else {
                SessionKind::Main
            },
            parent_session_id: parent.map(str::to_owned),
            agent_type: None,
        }
    }

    fn detail(id: &str, parent: Option<&str>, messages: Vec<SessionMessage>) -> Arc<SessionDetail> {
        Arc::new(SessionDetail {
            subtree_usage: None,
            session: session(id, parent),
            messages,
        })
    }

    fn options(language: Language) -> ExportOptions {
        ExportOptions {
            language,
            chat_bubbles: false,
            show_thinking: true,
            collapse_tools: true,
        }
    }

    #[test]
    fn renders_escaped_messages() {
        let mut message = SessionMessage::text(
            MessageType::User,
            "2026-01-01T00:00:00Z",
            "<img src=x onerror=alert(1)> & \"quotes\"",
        );
        message.metadata = serde_json::json!({ "html": "<script>" })
            .as_object()
            .unwrap()
            .clone();
        let bundle = ExportBundle {
            details: vec![detail("main", None, vec![message])],
            missing_children: Vec::new(),
        };
        let files = render_bundle(
            &bundle,
            options(Language::Zh),
            "session.html",
            "session_subagents",
        );

        assert!(files[0].html.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(files[0].html.contains("&lt;script&gt;"));
        assert!(!files[0].html.contains("<img src=x"));
        assert!(files[0].html.contains("name=\"export-filter\""));
        assert!(
            files[0]
                .html
                .contains("#export-filter-user:checked~main .reply-row")
        );
        assert!(
            files[0].html.contains(
                "#export-filter-user:checked~.filter-bar label[for=\"export-filter-user\"]"
            )
        );
        assert!(files[0].html.contains("class=\"user-row\""));
        assert!(files[0].html.contains("class=\"user-bubble\""));
    }

    #[test]
    fn links_parent_and_subagent_pages_with_relative_paths() {
        let mut parent_message =
            SessionMessage::text(MessageType::ToolUse, "2026-01-01T00:00:00Z", "task");
        parent_message.sub_agent_session_id = Some("child".to_owned());
        let bundle = ExportBundle {
            details: vec![
                detail("main", None, vec![parent_message]),
                detail("child", Some("main"), Vec::new()),
            ],
            missing_children: Vec::new(),
        };
        let files = render_bundle(
            &bundle,
            options(Language::En),
            "session.html",
            "session_subagents",
        );

        assert!(files[0].html.contains("session_subagents/agent-1.html"));
        assert!(files[1].html.contains("../session.html"));
    }

    #[test]
    fn hidden_thinking_is_not_exported() {
        let mut message = SessionMessage::text(MessageType::Assistant, "t", "final");
        message.reasoning_content = Some("private reasoning".to_owned());
        let bundle = ExportBundle {
            details: vec![detail("main", None, vec![message])],
            missing_children: Vec::new(),
        };
        let mut hidden = options(Language::Zh);
        hidden.show_thinking = false;
        let files = render_bundle(&bundle, hidden, "session.html", "session_subagents");
        assert!(!files[0].html.contains("private reasoning"));
        assert!(!files[0].html.contains("思考过程"));

        let files = render_bundle(
            &bundle,
            options(Language::Zh),
            "session.html",
            "session_subagents",
        );
        assert!(files[0].html.contains("private reasoning"));
    }

    #[test]
    fn remote_markdown_images_are_omitted() {
        let message = SessionMessage::text(
            MessageType::Assistant,
            "t",
            "![chart](https://example.com/track.png)",
        );
        let bundle = ExportBundle {
            details: vec![detail("main", None, vec![message])],
            missing_children: Vec::new(),
        };
        let files = render_bundle(
            &bundle,
            options(Language::Zh),
            "session.html",
            "session_subagents",
        );
        assert!(!files[0].html.contains("<img"));
        assert!(files[0].html.contains("远端图片已省略"));
    }

    #[test]
    fn hrefs_encode_path_special_characters() {
        let name = safe_component("report#1?v2%.html", 180);
        assert!(!name.contains('#'));
        assert!(!name.contains('?'));
        assert!(!name.contains('%'));
        assert_eq!(href_segment("a#b?c"), "a%23b%3Fc");
    }

    #[test]
    fn tool_output_fallback_matches_app_renderer() {
        let mut message = SessionMessage::text(MessageType::ToolUse, "t", "");
        message.tool_name = Some("read".to_owned());
        message.tool_output = Some(ToolOutput {
            output: None,
            preview: None,
            truncated: false,
            extra: serde_json::json!({
                "content": [{ "type": "text", "text": "fallback-output" }]
            })
            .as_object()
            .unwrap()
            .clone(),
        });
        let bundle = ExportBundle {
            details: vec![detail("main", None, vec![message])],
            missing_children: Vec::new(),
        };
        let files = render_bundle(
            &bundle,
            options(Language::En),
            "session.html",
            "session_subagents",
        );
        assert!(files[0].html.contains("fallback-output"));
    }
}
