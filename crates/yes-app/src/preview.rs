use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::PathBuf,
    sync::Arc,
};

use crate::{
    i18n::tr,
    preview_selection::{CodeSelection, CodeText},
    preview_syntax::{TokenKind, highlight_lines},
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    scroll::{Scrollbar, ScrollbarMode},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use yes_core::{
    Language,
    workspace::{self, Change, ChangeScope, FileContent, PreviewImageFormat},
};

actions!(preview, [CopyCode, SelectAllCode]);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PreviewTab {
    Files,
    Changes,
}

#[derive(Clone)]
struct FileRow {
    header: bool,
    depth: usize,
    path: PathBuf,
    directory: bool,
    ignored: bool,
    scope: Option<ChangeScope>,
    status: String,
    stats: Option<(usize, usize)>,
}
#[derive(Clone, Default)]
struct CodeRow {
    old: Option<usize>,
    new: Option<usize>,
    text: String,
    original_text: Option<String>,
    kind: char,
    syntax: Vec<(Range<usize>, TokenKind)>,
    old_syntax: Vec<(Range<usize>, TokenKind)>,
    changed: Option<Range<usize>>,
    folded: Option<Range<usize>>,
}

pub struct WorkspacePreview {
    root: PathBuf,
    selection: Entity<CodeSelection>,
    change_count: Option<usize>,
    changes: Vec<Change>,
    branch: Option<String>,
    count_generation: u64,
    changes_loaded: bool,
    changes_loading: bool,
    changes_error: Option<String>,
    search_generation: u64,
    last_scope: Option<ChangeScope>,
    language: Language,
    search_language: Language,
    tab: PreviewTab,
    tree_children: HashMap<PathBuf, Vec<FileRow>>,
    tree_expanded: HashSet<PathBuf>,
    tree_loading: HashSet<PathBuf>,
    tree_errors: HashMap<PathBuf, String>,
    tree_generation: u64,
    search: Entity<InputState>,
    _subscription: Subscription,
    rows: Vec<FileRow>,
    change_files: Vec<FileRow>,
    change_collapsed: HashSet<(ChangeScope, PathBuf)>,
    selected: Option<(PathBuf, Option<ChangeScope>)>,
    code: Vec<CodeRow>,
    full_code: Vec<CodeRow>,
    expanded_context: HashSet<usize>,
    split_rows: Vec<(Option<usize>, Option<usize>)>,
    code_width: f32,
    diff_stats: (usize, usize),
    image: Option<Arc<Image>>,
    image_diff: Option<(ImageSide, ImageSide)>,
    change_signature: Option<u64>,
    checking_changes: bool,
    changes_available: bool,
    signature_generation: u64,
    list_visible: bool,
    side_by_side: bool,
    list_started: bool,
    loading_list: bool,
    loading_content: bool,
    list_error: Option<String>,
    content_error: Option<String>,
    list_generation: u64,
    content_generation: u64,
    highlight_generation: u64,
    scroll: UniformListScrollHandle,
    list_scroll: UniformListScrollHandle,
    horizontal_scroll: ScrollHandle,
    right_horizontal_scroll: ScrollHandle,
}

impl WorkspacePreview {
    pub fn new(
        root: PathBuf,
        language: Language,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.bind_keys([
            KeyBinding::new("cmd-c", CopyCode, Some("CodePreview")),
            KeyBinding::new("cmd-a", SelectAllCode, Some("CodePreview")),
        ]);
        let selection = cx.new(|cx| CodeSelection::new(cx));
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr(language, "preview.search")));
        let subscription = cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.search_generation += 1;
                let generation = this.search_generation;
                this.list_generation += 1;
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(200))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.search_generation == generation {
                            this.load_list(cx);
                        }
                    });
                })
                .detach();
            }
        });
        let mut this = Self {
            root,
            selection,
            change_count: None,
            changes: vec![],
            branch: None,
            count_generation: 0,
            changes_loaded: false,
            changes_loading: false,
            changes_error: None,
            search_generation: 0,
            last_scope: None,
            language,
            search_language: language,
            tab: PreviewTab::Files,
            tree_children: HashMap::new(),
            tree_expanded: HashSet::new(),
            tree_loading: HashSet::new(),
            tree_errors: HashMap::new(),
            tree_generation: 0,
            search,
            _subscription: subscription,
            rows: vec![],
            change_files: vec![],
            change_collapsed: HashSet::new(),
            selected: None,
            code: vec![],
            full_code: vec![],
            expanded_context: HashSet::new(),
            split_rows: vec![],
            code_width: 128.,
            diff_stats: (0, 0),
            image: None,
            image_diff: None,
            change_signature: None,
            checking_changes: false,
            changes_available: false,
            signature_generation: 0,
            list_visible: true,
            side_by_side: true,
            list_started: false,
            loading_list: false,
            loading_content: false,
            list_error: None,
            content_error: None,
            list_generation: 0,
            content_generation: 0,
            highlight_generation: 0,
            scroll: UniformListScrollHandle::new(),
            list_scroll: UniformListScrollHandle::new(),
            horizontal_scroll: ScrollHandle::new(),
            right_horizontal_scroll: ScrollHandle::new(),
        };
        this.load_count(cx);
        this.check_for_changes(cx);
        this
    }

    pub fn check_for_changes(&mut self, cx: &mut Context<Self>) {
        if self.checking_changes {
            return;
        }
        self.checking_changes = true;
        let generation = self.signature_generation;
        let root = self.root.clone();
        let task = cx
            .background_executor()
            .spawn(async move { workspace::change_signature(&root) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.signature_generation {
                    return;
                }
                this.checking_changes = false;
                if let Ok(signature) = result {
                    if let Some(previous) = this.change_signature {
                        this.changes_available = previous != signature;
                    } else {
                        this.change_signature = Some(signature);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn change_count(&self) -> Option<usize> {
        self.change_count
    }

    pub fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        if self.language != language {
            self.language = language;
            cx.notify();
        }
    }

    fn load_count(&mut self, cx: &mut Context<Self>) {
        if self.changes_loaded || self.changes_loading {
            return;
        }
        self.changes_loading = true;
        self.count_generation += 1;
        let generation = self.count_generation;
        self.change_count = None;
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move {
            (
                workspace::changes(&root).map_err(|error| error.to_string()),
                workspace::branch_name(&root).ok(),
            )
        });
        cx.spawn(async move |this, cx| {
            let (count, branch) = task.await;
            let _ = this.update(cx, |this, cx| {
                if generation == this.count_generation {
                    this.changes_loaded = true;
                    this.changes_loading = false;
                    this.change_count = count.as_ref().ok().map(|changes| {
                        changes
                            .iter()
                            .map(|change| &change.path)
                            .collect::<std::collections::HashSet<_>>()
                            .len()
                    });
                    let error = count.as_ref().err().cloned();
                    this.changes_error = error.clone();
                    this.changes = count.unwrap_or_default();
                    this.branch = branch;
                    if this.tab == PreviewTab::Changes {
                        this.loading_list = false;
                        this.rebuild_changes(cx);
                        this.list_error = error;
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn set_tab(&mut self, tab: PreviewTab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.list_scroll = UniformListScrollHandle::new();
        }
        self.tab = tab;
        self.list_visible = true;
        self.load_list(cx);
        self.load_count(cx);
    }

    pub fn return_to_list(&mut self, cx: &mut Context<Self>) -> bool {
        if self.list_visible {
            return false;
        }
        self.list_visible = true;
        cx.notify();
        true
    }

    pub fn open_path(&mut self, path: PathBuf, line: Option<usize>, cx: &mut Context<Self>) {
        self.tab = PreviewTab::Files;
        self.load_list(cx);
        self.load_count(cx);
        let relative = if path.is_absolute() {
            path.strip_prefix(&self.root)
                .map(PathBuf::from)
                .unwrap_or(path)
        } else {
            path
        };
        self.load_content(relative, None, line, cx);
    }

    pub fn reveal(&mut self, cx: &mut Context<Self>) {
        if !self.list_started {
            self.load_list(cx);
        }
    }

    fn rebuild_changes(&mut self, cx: &App) {
        let query = self.search.read(cx).value().trim().to_lowercase();
        self.change_files = self
            .changes
            .iter()
            .filter(|entry| entry.path.to_string_lossy().to_lowercase().contains(&query))
            .map(|entry| FileRow {
                header: false,
                depth: 0,
                path: entry.path.clone(),
                directory: false,
                ignored: false,
                scope: Some(entry.scope),
                status: entry.status.clone(),
                stats: entry.additions.zip(entry.deletions),
            })
            .collect();
        self.rows = change_tree_rows(&self.change_files, &self.change_collapsed);
    }

    fn load_list(&mut self, cx: &mut Context<Self>) {
        self.list_started = true;
        self.list_generation += 1;
        let generation = self.list_generation;
        if self.tab == PreviewTab::Changes {
            self.loading_list = !self.changes_loaded;
            self.list_error = self.changes_error.clone();
            self.rebuild_changes(cx);
            self.load_count(cx);
            cx.notify();
            return;
        }
        if self.tab == PreviewTab::Files && self.search.read(cx).value().trim().is_empty() {
            self.list_error = None;
            self.rebuild_tree();
            self.load_directory(PathBuf::new(), cx);
            cx.notify();
            return;
        }
        self.loading_list = true;
        self.list_error = None;
        let root = self.root.clone();
        let query = self.search.read(cx).value().to_string();
        let task = cx.background_executor().spawn(async move {
            let result = {
                workspace::search(&root, &query).map(|entries| {
                    entries
                        .into_iter()
                        .map(|entry| FileRow {
                            header: false,
                            depth: 0,
                            path: entry.path,
                            directory: entry.is_dir,
                            ignored: entry.ignored,
                            scope: None,
                            status: String::new(),
                            stats: None,
                        })
                        .collect()
                })
            };
            result.map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.list_generation {
                    return;
                }
                this.loading_list = false;
                match result {
                    Ok(rows) => {
                        if this.tab == PreviewTab::Changes {
                            this.change_files = rows;
                            this.rows =
                                change_tree_rows(&this.change_files, &this.change_collapsed);
                        } else {
                            this.rows = rows;
                        }
                    }
                    Err(error) => {
                        this.rows.clear();
                        this.list_error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn rebuild_tree(&mut self) {
        fn append(
            path: &PathBuf,
            depth: usize,
            children: &HashMap<PathBuf, Vec<FileRow>>,
            expanded: &HashSet<PathBuf>,
            parent_ignored: bool,
            rows: &mut Vec<FileRow>,
        ) {
            if let Some(entries) = children.get(path) {
                for entry in entries {
                    let mut row = entry.clone();
                    row.depth = depth;
                    row.ignored |= parent_ignored;
                    let ignored = row.ignored;
                    rows.push(row);
                    if entry.directory && expanded.contains(&entry.path) {
                        append(&entry.path, depth + 1, children, expanded, ignored, rows);
                    }
                }
            }
        }
        self.rows.clear();
        append(
            &PathBuf::new(),
            0,
            &self.tree_children,
            &self.tree_expanded,
            false,
            &mut self.rows,
        );
        self.loading_list = self.tree_loading.contains(&PathBuf::new());
        self.list_error = self.tree_errors.get(&PathBuf::new()).cloned();
    }

    fn toggle_directory(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.tree_expanded.remove(&path) {
            self.tree_expanded.insert(path.clone());
            self.load_directory(path, cx);
        }
        self.rebuild_tree();
        cx.notify();
    }

    fn load_directory(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.tree_children.contains_key(&path) || !self.tree_loading.insert(path.clone()) {
            return;
        }
        self.tree_errors.remove(&path);
        self.loading_list = path.as_os_str().is_empty();
        let generation = self.tree_generation;
        let root = self.root.clone();
        let directory = path.clone();
        let task = cx.background_executor().spawn(async move {
            workspace::entries(&root, &directory).map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.tree_generation {
                    return;
                }
                this.tree_loading.remove(&path);
                match result {
                    Ok(entries) => {
                        this.tree_children.insert(
                            path,
                            entries
                                .into_iter()
                                .map(|entry| FileRow {
                                    header: false,
                                    depth: 0,
                                    path: entry.path,
                                    directory: entry.is_dir,
                                    ignored: entry.ignored,
                                    scope: None,
                                    status: String::new(),
                                    stats: None,
                                })
                                .collect(),
                        );
                    }
                    Err(error) => {
                        this.tree_errors.insert(path, error);
                    }
                }
                if this.tab == PreviewTab::Files && this.search.read(cx).value().trim().is_empty() {
                    this.rebuild_tree();
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_content(
        &mut self,
        path: PathBuf,
        scope: Option<ChangeScope>,
        line: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        self.content_generation += 1;
        let generation = self.content_generation;
        if self
            .selected
            .as_ref()
            .is_none_or(|(selected, _)| selected != &path)
        {
            self.last_scope = None;
        }
        if scope.is_some() {
            self.last_scope = scope;
        }
        self.selected = Some((path.clone(), scope));
        self.list_visible = false;
        self.loading_content = true;
        self.content_error = None;
        self.code.clear();
        self.selection
            .update(cx, |state, _| state.reset(Default::default()));
        self.full_code.clear();
        self.expanded_context.clear();
        self.split_rows.clear();
        self.diff_stats = (0, 0);
        self.image = None;
        self.image_diff = None;
        self.scroll = UniformListScrollHandle::new();
        self.horizontal_scroll = ScrollHandle::new();
        self.right_horizontal_scroll = ScrollHandle::new();
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move {
            if let Some(scope) = scope {
                if let Some((before, after)) =
                    workspace::image_diff(&root, &path, scope).map_err(|e| e.to_string())?
                {
                    return Ok((
                        vec![],
                        None,
                        Some((
                            ImageSide::from_content(before),
                            ImageSide::from_content(after),
                        )),
                    ));
                }
            }
            let result = if let Some(scope) = scope {
                workspace::diff_with_context(&root, &path, scope, 100_000)
                    .or_else(|_| workspace::diff(&root, &path, scope))
                    .map(|patch| {
                        (
                            parse_diff(&patch)
                                .into_iter()
                                .filter(|row| {
                                    !["diff --git ", "index ", "--- ", "+++ "].iter().any(
                                        |prefix| row.kind == '@' && row.text.starts_with(prefix),
                                    )
                                })
                                .collect(),
                            None,
                        )
                    })
                    .map_err(|error| error.to_string())
            } else {
                workspace::read_file(&root, &path)
                    .map(|content| match content {
                        FileContent::Text(text) => (
                            text.lines()
                                .enumerate()
                                .map(|(index, text)| CodeRow {
                                    old: None,
                                    new: Some(index + 1),
                                    text: text.to_owned(),
                                    kind: ' ',
                                    syntax: Vec::new(),
                                    old_syntax: Vec::new(),
                                    ..Default::default()
                                })
                                .collect(),
                            None,
                        ),
                        FileContent::Image { format, bytes } => {
                            let format = image_format(format);
                            (vec![], Some(Arc::new(Image::from_bytes(format, bytes))))
                        }
                        FileContent::Unsupported => (
                            vec![CodeRow {
                                text: "\0unsupported".into(),
                                ..Default::default()
                            }],
                            None,
                        ),
                    })
                    .map_err(|error| error.to_string())
            };
            result.map(|(mut code, image): (Vec<CodeRow>, _)| {
                for row in &mut code {
                    if row.text.contains('\t') {
                        row.original_text = Some(row.text.clone());
                    }
                    row.text = row.text.replace('\t', "    ");
                }
                if scope.is_some() {
                    mark_inline_changes(&mut code);
                } else {
                    color_code(&path, &mut code, false);
                }
                (code, image, None)
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.content_generation {
                    return;
                }
                this.loading_content = false;
                match result {
                    Ok((code, image, image_diff)) => {
                        this.image_diff = image_diff;
                        this.diff_stats = (
                            code.iter().filter(|row| row.kind == '+').count(),
                            code.iter().filter(|row| row.kind == '-').count(),
                        );
                        this.code_width = code
                            .iter()
                            .map(|row| {
                                row.text
                                    .chars()
                                    .map(|c| if c.is_ascii() { 8. } else { 16. })
                                    .sum::<f32>()
                            })
                            .fold(0., f32::max)
                            + 128.;
                        this.full_code = code;
                        this.rebuild_context(cx);
                        this.highlight_visible(cx);
                        this.image = image;
                        if let Some(line) = line {
                            this.scroll.scroll_to_item(
                                line.saturating_sub(1)
                                    .min(this.code.len().saturating_sub(1)),
                                ScrollStrategy::Top,
                            );
                        }
                    }
                    Err(error) => this.content_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn highlight_visible(&mut self, cx: &mut Context<Self>) {
        self.highlight_generation += 1;
        let highlight_generation = self.highlight_generation;
        let content_generation = self.content_generation;
        let Some((path, Some(_))) = self.selected.clone() else {
            return;
        };
        let mut code = self.code.clone();
        let task = cx.background_executor().spawn(async move {
            color_code(&path, &mut code, true);
            code
        });
        cx.spawn(async move |this, cx| {
            let code = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.content_generation == content_generation
                    && this.highlight_generation == highlight_generation
                {
                    this.code = code;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn rebuild_context(&mut self, cx: &mut Context<Self>) {
        self.code = if self
            .selected
            .as_ref()
            .is_some_and(|(_, scope)| scope.is_some())
        {
            fold_context(&self.full_code, &self.expanded_context)
        } else {
            self.full_code.clone()
        };
        self.split_rows = pair_diff_rows(&self.code);
        let text = |row: &CodeRow| {
            if row.folded.is_some() || row.kind == '@' {
                return None;
            }
            Some(
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|(_, scope)| scope.is_some())
                    && (row.old.is_some() || row.new.is_some())
                {
                    row.text.get(1..).unwrap_or_default().to_owned()
                } else {
                    row.text.clone()
                },
            )
        };
        let documents = [
            self.code.iter().map(text).collect(),
            self.split_rows
                .iter()
                .map(|(left, _)| left.and_then(|i| text(&self.code[i])))
                .collect(),
            self.split_rows
                .iter()
                .map(|(_, right)| right.and_then(|i| text(&self.code[i])))
                .collect(),
        ];
        let original = |row: &CodeRow| {
            row.original_text.as_ref().map(|text| {
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|(_, scope)| scope.is_some())
                    && (row.old.is_some() || row.new.is_some())
                {
                    text.get(1..).unwrap_or_default().to_owned()
                } else {
                    text.clone()
                }
            })
        };
        let originals = [
            self.code.iter().map(original).collect(),
            self.split_rows
                .iter()
                .map(|(left, _)| left.and_then(|i| original(&self.code[i])))
                .collect(),
            self.split_rows
                .iter()
                .map(|(_, right)| right.and_then(|i| original(&self.code[i])))
                .collect(),
        ];
        self.selection.update(cx, |state, _| {
            state.reset(documents);
            state.originals = originals;
            state.clear(
                if self.side_by_side
                    && self
                        .selected
                        .as_ref()
                        .is_some_and(|(_, scope)| scope.is_some())
                {
                    1
                } else {
                    0
                },
            );
        });
    }

    fn toggle_change_directory(
        &mut self,
        scope: ChangeScope,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let key = (scope, path);
        if !self.change_collapsed.remove(&key) {
            self.change_collapsed.insert(key);
        }
        self.rows = change_tree_rows(&self.change_files, &self.change_collapsed);
        cx.notify();
    }

    fn render_code_panel(&self, side: Option<bool>, cx: &mut Context<Self>) -> AnyElement {
        let is_diff = self
            .selected
            .as_ref()
            .is_some_and(|(_, scope)| scope.is_some());
        let horizontal = if side == Some(false) {
            &self.right_horizontal_scroll
        } else {
            &self.horizontal_scroll
        };
        let code = uniform_list(
            if side == Some(false) {
                "preview-code-right"
            } else {
                "preview-code"
            },
            if side.is_some() {
                self.split_rows.len()
            } else {
                self.code.len()
            },
            cx.processor(move |this, range: Range<usize>, _, cx| {
                range
                    .map(|index| {
                        let row_index = match side {
                            Some(true) => this.split_rows[index].0,
                            Some(false) => this.split_rows[index].1,
                            None => Some(index),
                        };
                        let row = row_index.map(|index| &this.code[index]);
                        if let Some(folded) = row.and_then(|row| row.folded.clone()) {
                            let start = folded.start;
                            return div()
                                .id(("diff-context", index))
                                .debug_selector(|| "diff-context".into())
                                .h(px(24.))
                                .w_full()
                                .px_3()
                                .text_size(px(12.))
                                .bg(cx.theme().muted)
                                .text_color(cx.theme().muted_foreground)
                                .hover(|style| style.bg(cx.theme().accent))
                                .cursor_pointer()
                                .child(tr(this.language, "preview.expandContext").replace(
                                    "{count}",
                                    &folded.len().saturating_sub(6).to_string(),
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.expanded_context.insert(start);
                                    this.rebuild_context(cx);
                                    this.highlight_visible(cx);
                                    cx.notify();
                                }))
                                .into_any_element();
                        }
                        let kind = row.map_or(' ', |row| row.kind);
                        let accent = match kind {
                            '+' => hsla(0.40, 0.55, 0.48, 1.),
                            '-' => hsla(0.96, 0.72, 0.56, 1.),
                            _ => cx.theme().muted_foreground,
                        };
                        let background = if row.is_none() {
                            cx.theme().muted.opacity(0.35)
                        } else if matches!(kind, '+' | '-') {
                            accent.opacity(0.13)
                        } else if kind == '@' {
                            cx.theme().muted.opacity(0.65)
                        } else {
                            cx.theme().background
                        };
                        let number = |value: Option<usize>| {
                            div()
                                .w(px(48.))
                                .h_full()
                                .flex_none()
                                .pr_2()
                                .text_right()
                                .text_color(accent)
                                .child(value.map(|value| value.to_string()).unwrap_or_default())
                        };
                        div()
                            .h(px(24.))
                            .w_full()
                            .flex()
                            .items_center()
                            .whitespace_nowrap()
                            .text_size(px(13.))
                            .font_family("Menlo")
                            .bg(background)
                            .border_l_4()
                            .border_color(if matches!(kind, '+' | '-') {
                                accent
                            } else {
                                gpui_kit::transparent_black()
                            })
                            .when_some(row, |element, row| {
                                let text = if is_diff && (row.old.is_some() || row.new.is_some()) {
                                    row.text.get(1..).unwrap_or_default().to_owned()
                                } else {
                                    row.text.clone()
                                };
                                element
                                    .when(side.is_none() && is_diff, |element| {
                                        element.child(number(row.old))
                                    })
                                    .child(number(if side == Some(true) {
                                        row.old
                                    } else {
                                        row.new
                                    }))
                                    .child(
                                        div()
                                            .debug_selector(move || {
                                                format!(
                                                    "code-text-{}-{index}",
                                                    match side {
                                                        None => 0,
                                                        Some(true) => 1,
                                                        Some(false) => 2,
                                                    }
                                                )
                                                .into()
                                            })
                                            .h_full()
                                            .border_l_1()
                                            .border_color(cx.theme().border.opacity(0.5))
                                            .pl_3()
                                            .text_color(if kind == '@' {
                                                cx.theme().muted_foreground
                                            } else {
                                                cx.theme().foreground
                                            })
                                            .child(CodeText::new(
                                                StyledText::new(text.clone()).with_highlights(
                                                    row_highlights(
                                                        row,
                                                        side == Some(true),
                                                        accent,
                                                        is_diff,
                                                        cx,
                                                    ),
                                                ),
                                                text.len(),
                                                this.selection.clone(),
                                                match side {
                                                    None => 0,
                                                    Some(true) => 1,
                                                    Some(false) => 2,
                                                },
                                                index,
                                                cx.theme().selection,
                                            )),
                                    )
                            })
                            .into_any_element()
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .map(|mut list| {
            list.style().restrict_scroll_to_axis = Some(true);
            list
        })
        .w(px(self.code_width))
        .min_w_full()
        .h_full();
        div()
            .id(if side == Some(false) {
                "preview-right-pane"
            } else {
                "preview-left-pane"
            })
            .debug_selector(move || {
                if side == Some(false) {
                    "preview-right-pane".into()
                } else {
                    "preview-left-pane".into()
                }
            })
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .overflow_hidden()
            .when(side == Some(false), |panel| {
                panel.border_l_1().border_color(cx.theme().border)
            })
            .child(
                div()
                    .id(if side == Some(false) {
                        "preview-right-horizontal"
                    } else {
                        "preview-horizontal-scroll"
                    })
                    .size_full()
                    .overflow_x_scroll()
                    .map(|mut view| {
                        view.style().restrict_scroll_to_axis = Some(true);
                        view
                    })
                    .track_scroll(horizontal)
                    .child(code),
            )
            .child(Scrollbar::horizontal(horizontal).mode(ScrollbarMode::Always))
            .into_any_element()
    }

    fn scope_label(&self, scope: ChangeScope) -> &'static str {
        tr(
            self.language,
            match scope {
                ChangeScope::Staged => "preview.staged",
                ChangeScope::Unstaged => "preview.unstaged",
                ChangeScope::Untracked => "preview.untracked",
            },
        )
    }
}

impl Render for WorkspacePreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.language;
        if self.search_language != language {
            self.search_language = language;
            self.search.update(cx, |search, cx| {
                search.set_placeholder(tr(language, "preview.search"), window, cx)
            });
        }
        let mut toolbar = div()
            .flex()
            .items_center()
            .gap_1()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border);
        for (id, tab, label) in [
            ("preview-files", PreviewTab::Files, "preview.files"),
            ("preview-changes", PreviewTab::Changes, "preview.changesTab"),
        ] {
            toolbar = toolbar.child(
                Button::new(id)
                    .compact()
                    .label(if tab == PreviewTab::Changes {
                        self.change_count
                            .map(|count| format!("{} {count}", tr(language, label)))
                            .unwrap_or_else(|| tr(language, label).to_string())
                    } else {
                        tr(language, label).to_string()
                    })
                    .when(self.tab == tab, |button| button.primary())
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx))),
            );
        }
        toolbar = toolbar.child(div().flex_1()).child(
            Button::new("preview-refresh")
                .debug_selector(|| "preview-refresh".into())
                .ghost()
                .label(tr(language, "preview.refresh"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.count_generation += 1;
                    this.changes_loaded = false;
                    this.changes_loading = false;
                    this.signature_generation += 1;
                    this.change_signature = None;
                    this.checking_changes = false;
                    this.changes_available = false;
                    this.check_for_changes(cx);
                    this.tree_generation += 1;
                    this.tree_children.clear();
                    this.tree_expanded.clear();
                    this.tree_loading.clear();
                    this.tree_errors.clear();
                    this.load_list(cx);
                    this.load_count(cx);
                    if let Some((path, scope)) = this.selected.clone() {
                        let visible = this.list_visible;
                        this.load_content(path, scope, None, cx);
                        this.list_visible = visible;
                    }
                })),
        );
        let mut body = div()
            .key_context("CodePreview")
            .track_focus(&self.selection.read(cx).focus)
            .on_action(cx.listener(|this, _: &CopyCode, _, cx| {
                let text = this.selection.read(cx).copy();
                if !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }))
            .on_action(cx.listener(|this, _: &SelectAllCode, window, cx| {
                this.selection.update(cx, |state, _| state.select_all());
                window.refresh();
            }))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0();
        if self.changes_available {
            body = body.child(
                div()
                    .debug_selector(|| "preview-changes-available".into())
                    .flex_none()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .bg(cx.theme().selection.opacity(0.3))
                    .child(tr(language, "preview.changesAvailable")),
            );
        }
        if self.list_visible {
            if self.tab == PreviewTab::Changes {
                let (added, removed) = self
                    .changes
                    .iter()
                    .filter_map(|change| change.additions.zip(change.deletions))
                    .fold((0, 0), |(a, r), (next_a, next_r)| (a + next_a, r + next_r));
                body = body.child(
                    div()
                        .px_3()
                        .pt_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(tr(language, "preview.changesTab"))
                        .when_some(self.branch.clone(), |view, branch| {
                            view.child(
                                div()
                                    .min_w_0()
                                    .text_ellipsis()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(branch),
                            )
                        })
                        .child(change_stats(added, removed)),
                );
            }
            body = body.child(div().px_3().py_2().child(Input::new(&self.search)));
            if self.tab == PreviewTab::Files && !self.search.read(cx).value().trim().is_empty() {
                body = body.child(div().px_3().text_xs().child(tr(language, "preview.limit")));
            }
            if self.loading_list {
                body = body.child(div().p_4().child(tr(language, "preview.loading")));
            } else if let Some(error) = &self.list_error {
                body = body.child(
                    div()
                        .p_4()
                        .child(format!("{}: {error}", tr(language, "preview.error"))),
                );
            } else if self.rows.is_empty() {
                body = body.child(div().p_4().child(tr(
                    language,
                    if !self.search.read(cx).value().trim().is_empty() {
                        "preview.noFiles"
                    } else if self.tab == PreviewTab::Changes {
                        "preview.noChanges"
                    } else {
                        "preview.noFiles"
                    },
                )));
            } else {
                body = body.child(
                    uniform_list(
                        "preview-file-list",
                        self.rows.len(),
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            range
                                .map(|index| {
                                    let row = this.rows[index].clone();
                                    if row.header {
                                        let scope = row.scope.unwrap();
                                        let collapsed = this
                                            .change_collapsed
                                            .contains(&(scope, PathBuf::new()));
                                        return div()
                                            .id(("change-scope", index))
                                            .debug_selector(move || {
                                                format!("change-scope-header-{index}").into()
                                            })
                                            .w_full()
                                            .bg(cx.theme().muted.opacity(0.45))
                                            .hover(|style| style.bg(cx.theme().accent))
                                            .h(px(36.))
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .cursor_pointer()
                                            .text_size(px(14.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(cx.theme().foreground)
                                            .child(
                                                Icon::new(if collapsed {
                                                    IconName::ChevronRight
                                                } else {
                                                    IconName::ChevronDown
                                                })
                                                .size(px(12.)),
                                            )
                                            .child(this.scope_label(scope))
                                            .child(div().flex_1())
                                            .when_some(row.stats, |view, (a, r)| {
                                                view.child(change_stats(a, r))
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.toggle_change_directory(
                                                    scope,
                                                    PathBuf::new(),
                                                    cx,
                                                )
                                            }))
                                            .into_any_element();
                                    }
                                    let path = row.path.clone();
                                    let label = if this.tab == PreviewTab::Files
                                        && !this.search.read(cx).value().is_empty()
                                    {
                                        path.to_string_lossy().to_string()
                                    } else {
                                        path.file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .to_string()
                                    };
                                    let expanded = if let Some(scope) = row.scope {
                                        !this.change_collapsed.contains(&(scope, path.clone()))
                                    } else {
                                        this.tree_expanded.contains(&path)
                                    };
                                    let muted =
                                        row.ignored || hidden_tree_path(&path, row.directory);
                                    let foreground = if muted {
                                        cx.theme().muted_foreground
                                    } else {
                                        cx.theme().foreground
                                    };
                                    div()
                                        .id(("preview-entry", index))
                                        .debug_selector(move || {
                                            format!("preview-entry-{index}").into()
                                        })
                                        .w_full()
                                        .h(px(36.))
                                        .px_3()
                                        .pl(px(12.
                                            + (row.depth + usize::from(row.scope.is_some()))
                                                as f32
                                                * 16.))
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .cursor_pointer()
                                        .text_color(foreground)
                                        .text_size(px(13.))
                                        .hover(|style| style.bg(cx.theme().muted))
                                        .when(
                                            this.selected.as_ref()
                                                == Some(&(path.clone(), row.scope)),
                                            |view| view.bg(cx.theme().muted),
                                        )
                                        .child(
                                            div()
                                                .debug_selector(|| "change-entry-chevron".into())
                                                .w(px(12.))
                                                .flex_none()
                                                .when(row.directory, |view| {
                                                    view.child(
                                                        Icon::new(if expanded {
                                                            IconName::ChevronDown
                                                        } else {
                                                            IconName::ChevronRight
                                                        })
                                                        .size(px(12.)),
                                                    )
                                                }),
                                        )
                                        .child(
                                            Icon::new(if row.directory {
                                                IconName::Folder
                                            } else {
                                                IconName::FileText
                                            })
                                            .size(px(14.))
                                            .text_color(if row.scope.is_some() && !row.directory {
                                                status_color(&row.status, cx)
                                            } else {
                                                foreground
                                            }),
                                        )
                                        .when(this.tree_loading.contains(&path), |view| {
                                            view.child("…")
                                        })
                                        .when_some(this.tree_errors.get(&path), |view, error| {
                                            view.child(div().text_xs().child(format!(
                                                "{}: {error}",
                                                tr(this.language, "preview.error")
                                            )))
                                        })
                                        .child(div().flex_1().min_w_0().truncate().child(label))
                                        .when_some(row.stats, |view, (a, r)| {
                                            view.child(change_stats(a, r))
                                        })
                                        .when(row.scope.is_some() && !row.directory, |view| {
                                            view.child(
                                                div()
                                                    .flex_none()
                                                    .text_xs()
                                                    .text_color(status_color(&row.status, cx))
                                                    .child(row.status.clone()),
                                            )
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if row.directory {
                                                if let Some(scope) = row.scope {
                                                    this.toggle_change_directory(
                                                        scope,
                                                        path.clone(),
                                                        cx,
                                                    );
                                                } else {
                                                    this.toggle_directory(path.clone(), cx);
                                                }
                                            } else {
                                                this.load_content(
                                                    path.clone(),
                                                    row.scope,
                                                    None,
                                                    cx,
                                                );
                                            }
                                        }))
                                        .into_any_element()
                                })
                                .collect()
                        }),
                    )
                    .w_full()
                    .min_w_0()
                    .track_scroll(&self.list_scroll)
                    .map(|mut list| {
                        list.style().restrict_scroll_to_axis = Some(true);
                        list
                    })
                    .flex_1()
                    .min_h_0(),
                );
            }
        } else {
            body = body.child(
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("preview-return-to-tree")
                            .debug_selector(|| "preview-return-to-tree".into())
                            .ghost()
                            .w_full()
                            .h(px(32.))
                            .tooltip(tr(language, "preview.backShortcut"))
                            .accessibility_label(tr(
                                language,
                                if self.tab == PreviewTab::Files {
                                    "preview.backFiles"
                                } else {
                                    "preview.backChanges"
                                },
                            ))
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(Icon::new(IconName::ArrowLeft).size(px(16.)))
                                    .child(tr(
                                        language,
                                        if self.tab == PreviewTab::Files {
                                            "preview.backFiles"
                                        } else {
                                            "preview.backChanges"
                                        },
                                    ))
                                    .child(div().flex_1())
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Esc"),
                                    ),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.return_to_list(cx);
                            })),
                    ),
            );
            let mut heading = div()
                .debug_selector(|| "preview-actions".into())
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(cx.theme().border);
            if let Some((path, scope)) = self.selected.clone() {
                body = body.child(
                    div()
                        .px_3()
                        .pt_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .child(
                            div().flex_none().font_weight(FontWeight::SEMIBOLD).child(
                                path.file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                            ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    path.parent()
                                        .unwrap_or(std::path::Path::new(""))
                                        .to_string_lossy()
                                        .to_string(),
                                ),
                        )
                        .when(scope.is_some() && self.image_diff.is_none(), |view| {
                            view.child(change_stats(self.diff_stats.0, self.diff_stats.1))
                        }),
                );
                if let Some(scope) = scope {
                    body = body.child(
                        div()
                            .px_3()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.scope_label(scope)),
                    );
                }
                if scope.is_none() {
                    let mut scopes = self
                        .changes
                        .iter()
                        .filter(|change| change.path == path)
                        .map(|change| change.scope)
                        .collect::<Vec<_>>();
                    if scopes.is_empty() {
                        scopes.extend(self.last_scope);
                    }
                    for (index, change_scope) in scopes.into_iter().enumerate() {
                        let diff_path = path.clone();
                        heading = heading.child(
                            Button::new(("preview-file-diff", index))
                                .ghost()
                                .compact()
                                .label(format!(
                                    "{} · {}",
                                    self.scope_label(change_scope),
                                    tr(language, "preview.diff")
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.load_content(
                                        diff_path.clone(),
                                        Some(change_scope),
                                        None,
                                        cx,
                                    )
                                })),
                        );
                    }
                }
                if scope.is_some() {
                    heading = heading.child(
                        Button::new("preview-current")
                            .ghost()
                            .label(tr(language, "preview.current"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.load_content(path.clone(), None, None, cx)
                            })),
                    );
                    if self.image_diff.is_none() {
                        if !self.expanded_context.is_empty() {
                            heading = heading.child(
                                Button::new("collapse-diff-context")
                                    .debug_selector(|| "collapse-diff-context".into())
                                    .ghost()
                                    .label(tr(language, "preview.collapseContext"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.expanded_context.clear();
                                        this.rebuild_context(cx);
                                        this.highlight_visible(cx);
                                        this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                                        cx.notify();
                                    })),
                            );
                        }
                        heading = heading.child(
                            Button::new("preview-diff-layout")
                                .debug_selector(|| "preview-diff-layout".into())
                                .ghost()
                                .label(tr(
                                    language,
                                    if self.side_by_side {
                                        "preview.unified"
                                    } else {
                                        "preview.split"
                                    },
                                ))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.side_by_side = !this.side_by_side;
                                    this.selection.update(cx, |state, _| {
                                        state.clear(if this.side_by_side { 1 } else { 0 })
                                    });
                                    cx.notify();
                                })),
                        );
                    }
                }
            }
            if self.selected.as_ref().is_some_and(|(path, scope)| {
                scope.is_some()
                    || self.last_scope.is_some()
                    || self.changes.iter().any(|change| &change.path == path)
            }) {
                body = body.child(heading);
            }
            if self.loading_content {
                body = body.child(div().p_4().child(tr(language, "preview.loading")));
            } else if let Some(error) = &self.content_error {
                body = body.child(
                    div()
                        .p_4()
                        .child(format!("{}: {error}", tr(language, "preview.error"))),
                );
            } else if let Some((before, after)) = &self.image_diff {
                body = body.child(
                    div()
                        .debug_selector(|| "preview-image-diff".into())
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .child(image_diff_side(before, language, true, cx))
                        .child(image_diff_side(after, language, false, cx)),
                );
            } else if let Some(image) = &self.image {
                body = body.child(
                    div()
                        .id("preview-image")
                        .debug_selector(|| "preview-image".into())
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_hidden()
                        .p_4()
                        .child(
                            img(image.clone())
                                .size_full()
                                .object_fit(ObjectFit::Contain)
                                .with_loading(move || {
                                    preview_notice(language, "preview.loading").into_any_element()
                                })
                                .with_fallback(move || {
                                    preview_notice(language, "preview.imageError")
                                        .into_any_element()
                                }),
                        ),
                );
            } else if self
                .code
                .first()
                .is_some_and(|row| row.text == "\0unsupported")
            {
                body = body.child(
                    preview_notice(language, "preview.unsupported")
                        .id("preview-unsupported")
                        .debug_selector(|| "preview-unsupported".into()),
                );
            } else if self.code.is_empty() {
                body = body.child(div().p_4().child(tr(language, "preview.empty")));
            } else {
                let split = self.side_by_side
                    && self
                        .selected
                        .as_ref()
                        .is_some_and(|(_, scope)| scope.is_some());
                if split {
                    body = body.child(
                        div()
                            .flex()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .children(["preview.before", "preview.after"].map(|label| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .px_3()
                                    .py_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(tr(language, label))
                            })),
                    );
                }
                body = body.child(
                    div()
                        .id("preview-horizontal")
                        .debug_selector(|| "preview-code-viewport".into())
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .flex()
                        .child(self.render_code_panel(split.then_some(true), cx))
                        .when(split, |view| {
                            view.child(self.render_code_panel(Some(false), cx))
                        }),
                );
            }
        }
        div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_size(px(13.))
            .child(toolbar)
            .child(body)
    }
}

enum ImageSide {
    Missing,
    Unsupported,
    Image(Arc<Image>),
}

impl ImageSide {
    fn from_content(content: Option<FileContent>) -> Self {
        match content {
            None => Self::Missing,
            Some(FileContent::Image { format, bytes }) => {
                Self::Image(Arc::new(Image::from_bytes(image_format(format), bytes)))
            }
            _ => Self::Unsupported,
        }
    }
}

fn image_format(format: PreviewImageFormat) -> ImageFormat {
    match format {
        PreviewImageFormat::Png => ImageFormat::Png,
        PreviewImageFormat::Jpeg => ImageFormat::Jpeg,
        PreviewImageFormat::Gif => ImageFormat::Gif,
        PreviewImageFormat::Webp => ImageFormat::Webp,
        PreviewImageFormat::Bmp => ImageFormat::Bmp,
        PreviewImageFormat::Tiff => ImageFormat::Tiff,
        PreviewImageFormat::Ico => ImageFormat::Ico,
        PreviewImageFormat::Svg => ImageFormat::Svg,
    }
}

fn image_diff_side(side: &ImageSide, language: Language, before: bool, cx: &App) -> Div {
    let mut pane = div()
        .flex()
        .flex_col()
        .flex_1()
        .w_1_2()
        .h_full()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .border_l_1()
        .border_color(cx.theme().border)
        .child(div().p_2().text_sm().child(tr(
            language,
            if before {
                "preview.before"
            } else {
                "preview.after"
            },
        )));
    pane = pane.child(match side {
        ImageSide::Missing => preview_notice(
            language,
            if before {
                "preview.imageAdded"
            } else {
                "preview.imageDeleted"
            },
        )
        .into_any_element(),
        ImageSide::Unsupported => {
            preview_notice(language, "preview.unsupported").into_any_element()
        }
        ImageSide::Image(image) => div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .p_3()
            .child(
                img(image.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .with_loading(move || {
                        preview_notice(language, "preview.loading").into_any_element()
                    })
                    .with_fallback(move || {
                        preview_notice(language, "preview.imageError").into_any_element()
                    }),
            )
            .into_any_element(),
    });
    pane
}

fn preview_notice(language: Language, key: &str) -> Div {
    div()
        .size_full()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .p_4()
        .child(Icon::new(IconName::FileText).size(px(28.)))
        .child(
            div()
                .w_full()
                .text_center()
                .whitespace_normal()
                .text_size(px(13.))
                .child(tr(language, key)),
        )
}

fn color_code(path: &std::path::Path, code: &mut [CodeRow], diff: bool) {
    if !diff {
        let lines = code.iter().map(|row| row.text.clone()).collect::<Vec<_>>();
        for (row, spans) in code.iter_mut().zip(highlight_lines(path, &lines)) {
            row.syntax = spans;
        }
        return;
    }
    // Parse old/new streams separately, so deleted code cannot change the new side's state.
    // Reset at hunk boundaries because the omitted source may contain comment delimiters.
    let started = std::time::Instant::now();
    for hunk in code.split_mut(|row| matches!(row.kind, '@' | '.')) {
        if hunk.is_empty() {
            continue;
        }
        if started.elapsed() > std::time::Duration::from_millis(400) {
            break;
        }
        for old in [true, false] {
            let indices = hunk
                .iter()
                .enumerate()
                .filter(|(_, row)| {
                    if old {
                        row.old.is_some()
                    } else {
                        row.new.is_some()
                    }
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            let lines = indices
                .iter()
                .map(|index| hunk[*index].text.get(1..).unwrap_or_default().to_string())
                .collect::<Vec<_>>();
            for (index, spans) in indices.into_iter().zip(highlight_lines(path, &lines)) {
                if old {
                    hunk[index].old_syntax = spans.clone();
                    if hunk[index].new.is_none() {
                        hunk[index].syntax = spans;
                    }
                } else {
                    hunk[index].syntax = spans;
                }
            }
        }
    }
}

fn change_stats(added: usize, removed: usize) -> Div {
    div()
        .flex_none()
        .flex()
        .gap_2()
        .text_xs()
        .child(
            div()
                .text_color(hsla(0.40, 0.55, 0.48, 1.))
                .child(format!("+{added}")),
        )
        .child(
            div()
                .text_color(hsla(0.96, 0.72, 0.56, 1.))
                .child(format!("−{removed}")),
        )
}

fn status_color(status: &str, cx: &App) -> Hsla {
    match status {
        "A" | "?" | "??" => hsla(0.40, 0.55, 0.48, 1.),
        "D" => hsla(0.96, 0.72, 0.56, 1.),
        "M" | "R" | "C" => cx.theme().warning,
        _ => cx.theme().muted_foreground,
    }
}

fn change_tree_rows(
    files: &[FileRow],
    collapsed: &HashSet<(ChangeScope, PathBuf)>,
) -> Vec<FileRow> {
    fn append(
        files: &[&FileRow],
        scope: ChangeScope,
        parent: &std::path::Path,
        depth: usize,
        collapsed: &HashSet<(ChangeScope, PathBuf)>,
        rows: &mut Vec<FileRow>,
    ) {
        let mut directories = std::collections::BTreeMap::<PathBuf, Vec<&FileRow>>::new();
        let mut direct = Vec::new();
        for file in files {
            let relative = file.path.strip_prefix(parent).unwrap();
            if relative.components().count() > 1 {
                let path = parent.join(relative.components().next().unwrap().as_os_str());
                directories.entry(path).or_default().push(*file);
            } else {
                direct.push(*file);
            }
        }
        for (path, children) in directories {
            rows.push(FileRow {
                header: false,
                depth,
                path: path.clone(),
                directory: true,
                ignored: false,
                scope: Some(scope),
                status: String::new(),
                stats: None,
            });
            if !collapsed.contains(&(scope, path.clone())) {
                append(&children, scope, &path, depth + 1, collapsed, rows);
            }
        }
        direct.sort_by(|a, b| a.path.cmp(&b.path));
        rows.extend(direct.into_iter().map(|row| {
            let mut row = row.clone();
            row.depth = depth;
            row
        }));
    }
    let mut rows = Vec::new();
    for scope in [
        ChangeScope::Staged,
        ChangeScope::Unstaged,
        ChangeScope::Untracked,
    ] {
        let files = files
            .iter()
            .filter(|file| file.scope == Some(scope))
            .collect::<Vec<_>>();
        if files.is_empty() {
            continue;
        }
        let stats = files
            .iter()
            .filter_map(|file| file.stats)
            .fold((0, 0), |(a, r), (na, nr)| (a + na, r + nr));
        rows.push(FileRow {
            header: true,
            depth: 0,
            path: PathBuf::new(),
            directory: false,
            ignored: false,
            scope: Some(scope),
            status: String::new(),
            stats: files
                .iter()
                .any(|file| file.stats.is_some())
                .then_some(stats),
        });
        if !collapsed.contains(&(scope, PathBuf::new())) {
            append(
                &files,
                scope,
                std::path::Path::new(""),
                0,
                collapsed,
                &mut rows,
            );
        }
    }
    rows
}

fn hidden_tree_path(path: &std::path::Path, directory: bool) -> bool {
    let directories = if directory { Some(path) } else { path.parent() };
    directories.is_some_and(|path| path.components().any(|part| {
        matches!(part, std::path::Component::Normal(name) if name.to_string_lossy().starts_with('.'))
    }))
}

// Keep three unchanged lines on each side of an edit. Gaps retain indices into
// the loaded snapshot so expanding them never mixes in newer working-tree content.
fn fold_context(rows: &[CodeRow], expanded: &HashSet<usize>) -> Vec<CodeRow> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        if rows[index].kind != ' ' || rows[index].old.is_none() {
            result.push(rows[index].clone());
            index += 1;
            continue;
        }
        let start = index;
        while index < rows.len() && rows[index].kind == ' ' && rows[index].old.is_some() {
            index += 1;
        }
        let end = index;
        if end - start <= 6 || expanded.contains(&start) {
            result.extend_from_slice(&rows[start..end]);
        } else {
            result.extend_from_slice(&rows[start..start + 3]);
            result.push(CodeRow {
                kind: '.',
                // Include the visible context so the expansion key remains stable.
                folded: Some(start..end),
                ..Default::default()
            });
            result.extend_from_slice(&rows[end - 3..end]);
        }
    }
    result
}

fn changed_ranges(old: &str, new: &str) -> (Range<usize>, Range<usize>) {
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    let suffix = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    (prefix..old.len() - suffix, prefix..new.len() - suffix)
}

fn mark_inline_changes(rows: &mut [CodeRow]) {
    for (old, new) in pair_diff_rows(rows) {
        if let (Some(old), Some(new)) = (old, new) {
            if rows[old].kind == '-' && rows[new].kind == '+' {
                let (a, b) = changed_ranges(&rows[old].text[1..], &rows[new].text[1..]);
                rows[old].changed = Some(a);
                rows[new].changed = Some(b);
            }
        }
    }
}

fn row_highlights(
    row: &CodeRow,
    old_side: bool,
    accent: Hsla,
    is_diff: bool,
    cx: &App,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let syntax = if old_side {
        &row.old_syntax
    } else {
        &row.syntax
    };
    let mut boundaries = vec![
        0,
        row.text.len().saturating_sub(usize::from(
            is_diff && (row.old.is_some() || row.new.is_some()),
        )),
    ];
    for (range, _) in syntax {
        boundaries.extend([range.start, range.end]);
    }
    if let Some(range) = &row.changed {
        boundaries.extend([range.start, range.end]);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut syntax_index = 0;
    boundaries
        .windows(2)
        .filter_map(|bounds| {
            let range = bounds[0]..bounds[1];
            while syntax_index < syntax.len() && syntax[syntax_index].0.end <= range.start {
                syntax_index += 1;
            }
            let mut style = syntax
                .get(syntax_index)
                .filter(|(span, _)| span.contains(&range.start))
                .and_then(|(_, kind)| cx.theme().highlight_theme.style(kind.theme_key()))
                .unwrap_or_default();
            if row
                .changed
                .as_ref()
                .is_some_and(|span| span.contains(&range.start))
            {
                style.background_color = Some(accent.opacity(0.32));
            }
            (!range.is_empty()).then_some((range, style))
        })
        .collect()
}

// Pair adjacent deletion/addition runs without pairing across hunk boundaries.
fn pair_diff_rows(rows: &[CodeRow]) -> Vec<(Option<usize>, Option<usize>)> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        if matches!(rows[index].kind, '-' | '+') {
            let start = index;
            while index < rows.len() && rows[index].kind == '-' {
                index += 1;
            }
            let middle = index;
            while index < rows.len() && rows[index].kind == '+' {
                index += 1;
            }
            let count = (middle - start).max(index - middle);
            for offset in 0..count {
                result.push((
                    (start + offset < middle).then_some(start + offset),
                    (middle + offset < index).then_some(middle + offset),
                ));
            }
        } else {
            result.push((Some(index), Some(index)));
            index += 1;
        }
    }
    result
}

fn parse_diff(patch: &str) -> Vec<CodeRow> {
    let mut old = 0;
    let mut new = 0;
    let mut in_hunk = false;
    patch
        .lines()
        .map(|line| {
            if line.starts_with("@@ ") {
                let mut fields = line.split_whitespace();
                fields.next();
                old = fields
                    .next()
                    .and_then(|value| {
                        value
                            .trim_start_matches('-')
                            .split(',')
                            .next()?
                            .parse()
                            .ok()
                    })
                    .unwrap_or(0);
                new = fields
                    .next()
                    .and_then(|value| {
                        value
                            .trim_start_matches('+')
                            .split(',')
                            .next()?
                            .parse()
                            .ok()
                    })
                    .unwrap_or(0);
                in_hunk = true;
                return CodeRow {
                    text: line.into(),
                    kind: '@',
                    ..Default::default()
                };
            }
            if line.starts_with("diff --git") {
                in_hunk = false;
            }
            let kind = if in_hunk {
                line.chars().next().unwrap_or(' ')
            } else {
                '@'
            };
            let mut row = CodeRow {
                text: line.into(),
                kind,
                ..Default::default()
            };
            if in_hunk {
                if kind == ' ' || kind == '-' {
                    row.old = Some(old);
                    old += 1;
                }
                if kind == ' ' || kind == '+' {
                    row.new = Some(new);
                    new += 1;
                }
            }
            row
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{PreviewTab, WorkspacePreview, parse_diff};
    use gpui_kit::{TestAppContext, px, size};

    #[gpui_kit::test]
    fn code_selection_copies_across_rows_and_keeps_diff_sides_separate(cx: &mut TestAppContext) {
        use gpui_kit::{MouseButton, point};
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(800.), px(500.)), |window, cx| {
            WorkspacePreview::new(std::env::temp_dir(), yes_core::Language::En, window, cx)
        });
        let preview = window.root(cx).unwrap();
        preview.update(cx, |preview, cx| {
            preview.count_generation += 1;
            preview.selected = Some(("file.rs".into(), None));
            preview.list_visible = false;
            preview.full_code = (0..100)
                .map(|i| super::CodeRow {
                    text: format!("line {i}"),
                    new: Some(i + 1),
                    ..Default::default()
                })
                .collect();
            preview.rebuild_context(cx);
            cx.notify();
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        let first = visual.debug_bounds("code-text-0-0").unwrap();
        let third = visual.debug_bounds("code-text-0-2").unwrap();
        visual.simulate_mouse_down(
            point(first.left() + px(25.), first.center().y),
            MouseButton::Left,
            Default::default(),
        );
        visual.simulate_mouse_move(
            point(third.right() + px(10.), third.center().y),
            Some(MouseButton::Left),
            Default::default(),
        );
        visual.simulate_mouse_up(
            point(third.right() + px(10.), third.center().y),
            MouseButton::Left,
            Default::default(),
        );
        let copied = preview.read_with(cx, |preview, cx| preview.selection.read(cx).copy());
        assert!(copied.contains("line 1\nline 2"), "{copied:?}");
        visual.simulate_keystrokes("cmd-c");
        assert_eq!(
            cx.update(|cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            copied
        );
        visual.simulate_keystrokes("cmd-a cmd-c");
        assert!(
            cx.update(|cx| cx.read_from_clipboard().unwrap().text().unwrap())
                .ends_with("line 99")
        );
        preview.update(cx, |preview, cx| {
            preview.selection.update(cx, |state, _| state.select_all());
            assert!(preview.selection.read(cx).copy().ends_with("line 99"));
            preview.selected = Some((
                "file.rs".into(),
                Some(yes_core::workspace::ChangeScope::Unstaged),
            ));
            preview.full_code = parse_diff("@@ -1,2 +1,3 @@\n-old\n+new\n+extra\n same\n");
            preview.rebuild_context(cx);
            preview.side_by_side = true;
            cx.notify();
        });
        let right = visual.debug_bounds("code-text-2-1").unwrap();
        visual.simulate_click(right.center(), Default::default());
        preview.update(cx, |preview, cx| {
            preview.selection.update(cx, |state, _| state.select_all());
            let text = preview.selection.read(cx).copy();
            assert_eq!(text, "new\nextra\nsame");
        });
        let toggle = visual.debug_bounds("preview-diff-layout").unwrap();
        visual.simulate_click(toggle.center(), Default::default());
        preview.update(cx, |preview, cx| {
            assert!(preview.selection.read(cx).copy().is_empty());
            preview.selection.update(cx, |state, _| state.select_all());
            assert_eq!(preview.selection.read(cx).copy(), "old\nnew\nextra\nsame");
        });
    }

    #[test]
    fn inline_changes_preserve_unicode_and_context_expands_from_snapshot() {
        let (a, b) = super::changed_ranges("let 名称 = 旧值;", "let 名称 = 新值;");
        assert_eq!(&"let 名称 = 旧值;"[a], "旧");
        assert_eq!(&"let 名称 = 新值;"[b], "新");
        assert_eq!(super::changed_ranges("abc", "abc"), (3..3, 3..3));
        assert_eq!(super::changed_ranges("ab", "a新b"), (1..1, 1..4));
        let mut patch = String::from("@@ -1,22 +1,22 @@\n-old\n+new\n");
        for i in 2..=22 {
            patch.push_str(&format!(" context {i}\n"));
        }
        let mut rows = parse_diff(&patch);
        super::mark_inline_changes(&mut rows);
        assert_eq!(rows[1].changed, Some(0..3));
        let mut expanded = std::collections::HashSet::new();
        let collapsed = super::fold_context(&rows, &expanded);
        let gap = collapsed.iter().find_map(|row| row.folded.clone()).unwrap();
        assert_eq!(gap.len() - 6, 15);
        expanded.insert(gap.start);
        let full = super::fold_context(&rows, &expanded);
        assert_eq!(
            full.iter().map(|row| &row.text).collect::<Vec<_>>(),
            rows.iter().map(|row| &row.text).collect::<Vec<_>>()
        );
        assert_eq!(full.last().unwrap().new, Some(22));
        assert!(
            super::pair_diff_rows(&collapsed)
                .iter()
                .any(|(a, b)| a == b && a.is_some_and(|i| collapsed[i].folded.is_some()))
        );
    }

    #[gpui_kit::test]
    fn changes_scope_headers_and_children_fill_a_narrow_panel(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(360.), px(600.)), |window, cx| {
            WorkspacePreview::new(std::env::temp_dir(), yes_core::Language::Zh, window, cx)
        });
        let preview = window.root(cx).unwrap();
        cx.run_until_parked();
        preview.update(cx, |preview, cx| {
            preview.changes = [
                yes_core::workspace::ChangeScope::Staged,
                yes_core::workspace::ChangeScope::Unstaged,
                yes_core::workspace::ChangeScope::Untracked,
            ]
            .into_iter()
            .map(|scope| yes_core::workspace::Change {
                scope,
                path: "Business/deep/file.rs".into(),
                status: "M".into(),
                additions: Some(2),
                deletions: Some(1),
            })
            .collect();
            preview.changes_error = None;
            preview.changes_loaded = true;
            preview.tab = PreviewTab::Changes;
            preview.list_error = None;
            preview.loading_list = false;
            preview.rebuild_changes(cx);
            cx.notify();
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        let header = visual.debug_bounds("change-scope-header-0").unwrap();
        let entry = visual.debug_bounds("preview-entry-1").unwrap();
        let chevron = visual.debug_bounds("change-entry-chevron").unwrap();
        assert!(header.size.width >= px(340.));
        assert_eq!(header.size.width, entry.size.width);
        assert!(chevron.left() >= header.left() + px(28.));
        visual.simulate_click(header.center(), Default::default());
        cx.run_until_parked();
        preview.read_with(cx, |preview, _| {
            assert_eq!(preview.rows.iter().filter(|row| row.header).count(), 3);
            assert!(
                !preview.rows.iter().any(|row| !row.header
                    && row.scope == Some(yes_core::workspace::ChangeScope::Staged))
            );
        });
    }

    #[gpui_kit::test]
    fn diff_context_controls_expand_and_collapse_cached_rows(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(900.), px(600.)), |window, cx| {
            WorkspacePreview::new(std::env::temp_dir(), yes_core::Language::En, window, cx)
        });
        let preview = window.root(cx).unwrap();
        cx.run_until_parked();
        preview.update(cx, |preview, cx| {
            preview.selected = Some((
                "example.rs".into(),
                Some(yes_core::workspace::ChangeScope::Unstaged),
            ));
            preview.list_visible = false;
            let mut patch = String::from("@@ -1,30 +1,30 @@\n-old\n+new\n");
            for i in 2..=30 {
                patch.push_str(&format!(" line {i}\n"));
            }
            preview.full_code = parse_diff(&patch);
            super::mark_inline_changes(&mut preview.full_code);
            preview.rebuild_context(cx);
            cx.notify();
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        let folded_len = preview.read_with(cx, |preview, _| preview.code.len());
        let gap = visual.debug_bounds("diff-context").unwrap();
        visual.simulate_click(gap.center(), Default::default());
        cx.run_until_parked();
        preview.read_with(cx, |preview, _| {
            assert!(preview.code.len() > folded_len);
            assert_eq!(
                preview.content_generation, 0,
                "expansion must not reload content"
            );
            assert_eq!(preview.code.last().unwrap().new, Some(30));
        });
        let collapse = visual.debug_bounds("collapse-diff-context").unwrap();
        visual.simulate_click(collapse.center(), Default::default());
        cx.run_until_parked();
        assert_eq!(
            preview.read_with(cx, |preview, _| preview.code.len()),
            folded_len
        );
    }

    #[gpui_kit::test]
    fn change_notice_preserves_preview_and_image_diff_fits_panel(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!(
            "yes-change-notice-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(root.join("file.txt"), "initial").unwrap();
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(360.), px(600.)), |window, cx| {
            WorkspacePreview::new(root.clone(), yes_core::Language::En, window, cx)
        });
        let preview = window.root(cx).unwrap();
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        cx.run_until_parked();
        preview.update(cx, |preview, cx| {
            preview.open_path("file.txt".into(), None, cx)
        });
        cx.run_until_parked();
        let generation = preview.read_with(cx, |preview, _| preview.count_generation);
        preview.update(cx, |preview, cx| {
            preview.set_tab(PreviewTab::Changes, cx);
            preview.set_tab(PreviewTab::Files, cx);
            preview.load_count(cx);
            preview.list_visible = false;
        });
        cx.run_until_parked();
        assert_eq!(
            preview.read_with(cx, |preview, _| preview.count_generation),
            generation,
            "switching tabs must reuse the current Git snapshot"
        );
        std::fs::write(root.join("file.txt"), "new content from agent").unwrap();
        preview.update(cx, |preview, cx| preview.check_for_changes(cx));
        cx.run_until_parked();
        preview.read_with(cx, |preview, _| {
            assert!(preview.changes_available);
            assert_eq!(preview.code[0].text, "initial");
            assert!(!preview.list_visible);
        });
        assert!(visual.debug_bounds("preview-changes-available").is_some());
        preview.update(cx, |preview, cx| {
            preview.image_diff = Some((super::ImageSide::Missing, super::ImageSide::Unsupported));
            cx.notify();
        });
        let bounds = visual.debug_bounds("preview-image-diff").unwrap();
        assert!(bounds.size.width <= px(360.) && bounds.size.height > px(100.));
        let refresh = visual.debug_bounds("preview-refresh").unwrap();
        visual.simulate_click(refresh.center(), Default::default());
        cx.run_until_parked();
        preview.read_with(cx, |preview, _| {
            assert!(!preview.changes_available);
            assert_eq!(preview.code[0].text, "new content from agent");
        });
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui_kit::test]
    fn preview_scroll_locks_direction_and_back_preserves_tree(cx: &mut TestAppContext) {
        use gpui_kit::{ScrollDelta, ScrollWheelEvent, TouchPhase, point};
        let root = std::env::temp_dir();
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(600.), px(600.)), |window, cx| {
            WorkspacePreview::new(root, yes_core::Language::En, window, cx)
        });
        let preview = window.root(cx).unwrap();
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        for split in [false, true] {
            preview.update(cx, |preview, cx| {
                preview.selected = Some((
                    "main.rs".into(),
                    Some(yes_core::workspace::ChangeScope::Unstaged),
                ));
                preview.list_visible = false;
                preview.side_by_side = split;
                preview.code = parse_diff(&format!(
                    "@@ -0,0 +1,200 @@\n{}",
                    "+let value = 42;\n".repeat(200)
                ));
                preview.split_rows = super::pair_diff_rows(&preview.code);
                preview.code_width = 1200.;
                preview.scroll = gpui_kit::UniformListScrollHandle::new();
                preview.horizontal_scroll = gpui_kit::ScrollHandle::new();
                preview.right_horizontal_scroll = gpui_kit::ScrollHandle::new();
                preview.tree_expanded.insert("src".into());
                cx.notify();
            });
            let pane = visual.debug_bounds("preview-left-pane").unwrap();
            visual.simulate_mouse_move(pane.center(), None, Default::default());
            visual.simulate_event(ScrollWheelEvent {
                position: pane.center(),
                delta: ScrollDelta::Pixels(point(px(-1.), px(-40.))),
                modifiers: Default::default(),
                touch_phase: TouchPhase::Started,
            });
            visual.debug_bounds("preview-left-pane").unwrap();
            preview.read_with(cx, |preview, _| {
                assert_eq!(preview.horizontal_scroll.offset().x, px(0.));
                assert_eq!(preview.scroll.0.borrow().base_handle.offset().y, px(-40.));
            });
            visual.simulate_event(ScrollWheelEvent {
                position: pane.center(),
                delta: ScrollDelta::Pixels(point(px(-40.), px(-1.))),
                modifiers: Default::default(),
                touch_phase: TouchPhase::Started,
            });
            visual.debug_bounds("preview-left-pane").unwrap();
            preview.read_with(cx, |preview, _| {
                assert_eq!(preview.horizontal_scroll.offset().x, px(-40.));
                assert_eq!(preview.scroll.0.borrow().base_handle.offset().y, px(-40.));
            });
            let back = visual.debug_bounds("preview-return-to-tree").unwrap();
            visual.simulate_click(back.center(), Default::default());
            assert!(preview.read_with(cx, |preview, _| preview.list_visible
                && preview.tree_expanded.contains(std::path::Path::new("src"))));
        }
    }

    #[gpui_kit::test]
    fn image_preview_switches_to_unsupported_notice(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!(
            "yes-image-preview-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("image.svg"), "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"80\" height=\"40\"><rect width=\"80\" height=\"40\" fill=\"red\"/></svg>").unwrap();
        std::fs::write(root.join("document.pdf"), "%PDF-1.7\nASCII PDF").unwrap();
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(360.), px(600.)), |window, cx| {
            WorkspacePreview::new(root.clone(), yes_core::Language::En, window, cx)
        });
        let preview = window.root(cx).unwrap();
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        for language in [yes_core::Language::En, yes_core::Language::Zh] {
            preview.update(cx, |preview, cx| {
                preview.set_language(language, cx);
                preview.open_path("image.svg".into(), None, cx);
            });
            cx.run_until_parked();
            let image = visual.debug_bounds("preview-image").unwrap();
            assert!(image.size.width <= px(360.) && image.size.height > px(100.));
            assert!(preview.read_with(cx, |preview, _| preview.image.is_some()
                && preview.code.is_empty()));
            preview.update(cx, |preview, cx| {
                preview.open_path("document.pdf".into(), None, cx)
            });
            cx.run_until_parked();
            let notice = visual.debug_bounds("preview-unsupported").unwrap();
            assert!(notice.size.width <= px(360.) && notice.size.height > px(100.));
            assert!(preview.read_with(cx, |preview, _| preview.image.is_none()));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui_kit::test]
    fn preview_reads_files_and_rejects_stale_results(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!(
            "yes-ui-preview-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("first.rs"), "fn first() {}\r\n").unwrap();
        std::fs::write(root.join("second.rs"), "fn second() {}\n").unwrap();
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(360.), px(600.)), |window, cx| {
            WorkspacePreview::new(root.clone(), yes_core::Language::En, window, cx)
        });
        let preview = window.root(cx).unwrap();
        cx.run_until_parked();
        preview.update(cx, |preview, cx| preview.set_tab(PreviewTab::Files, cx));
        cx.run_until_parked();
        assert_eq!(preview.read_with(cx, |preview, _| preview.rows.len()), 2);
        preview.update(cx, |preview, cx| {
            preview.open_path("first.rs".into(), None, cx);
            preview.open_path("second.rs".into(), Some(1), cx);
        });
        cx.run_until_parked();
        assert_eq!(
            preview.read_with(cx, |preview, _| preview.code[0].text.clone()),
            "fn second() {}"
        );
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        for language in [yes_core::Language::En, yes_core::Language::Zh] {
            preview.update(cx, |preview, cx| preview.set_language(language, cx));
            cx.run_until_parked();
            let actions = visual.debug_bounds("preview-return-to-tree").unwrap();
            let code = visual.debug_bounds("preview-code-viewport").unwrap();
            assert!(actions.size.width <= px(360.));
            assert!(code.size.width <= px(360.) && code.size.height > px(100.));
        }
        preview.update(cx, |preview, cx| {
            preview.selected = Some((
                "second.rs".into(),
                Some(yes_core::workspace::ChangeScope::Unstaged),
            ));
            cx.notify();
        });
        cx.run_until_parked();
        let toggle = visual.debug_bounds("preview-diff-layout").unwrap();
        assert!(toggle.size.width > px(0.) && toggle.origin.x + toggle.size.width <= px(360.));
        // A long line must expose its full horizontal range at a narrow panel width.
        preview.update(cx, |preview, cx| {
            preview.code_width = 1200.;
            cx.notify();
        });
        cx.run_until_parked();
        visual.debug_bounds("preview-code-viewport").unwrap();
        let left = visual.debug_bounds("preview-left-pane").unwrap();
        let right = visual.debug_bounds("preview-right-pane").unwrap();
        assert!(left.size.width > px(100.) && right.size.width > px(100.));
        assert!((left.size.width - right.size.width).abs() <= px(1.));
        assert!(right.right() <= px(360.));
        preview.read_with(cx, |preview, _| {
            use gpui_kit::component::scroll::ScrollbarHandle;
            assert!(preview.horizontal_scroll.content_size().width >= px(1200.));
            assert!(preview.horizontal_scroll.viewport_bounds().size.width <= px(360.));
        });
        preview.update(cx, |preview, cx| {
            preview.open_path("../outside".into(), None, cx)
        });
        cx.run_until_parked();
        assert!(
            preview.read_with(cx, |preview, _| preview.content_error.is_some()
                && preview.code.is_empty())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui_kit::test]
    fn file_tree_loads_one_level_and_reuses_collapsed_children(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!(
            "yes-ui-tree-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::write(root.join("src/first.rs"), "first").unwrap();
        std::fs::write(root.join("src/nested/deep.rs"), "deep").unwrap();
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(400.), px(600.)), |window, cx| {
            WorkspacePreview::new(root.clone(), yes_core::Language::En, window, cx)
        });
        let preview = window.root(cx).unwrap();
        cx.run_until_parked();
        preview.update(cx, |preview, cx| preview.reveal(cx));
        cx.run_until_parked();
        assert!(preview.read_with(cx, |preview, _| preview.rows.len() == 1
            && preview.tree_children.len() == 1));
        preview.update(cx, |preview, cx| preview.toggle_directory("src".into(), cx));
        cx.run_until_parked();
        assert!(preview.read_with(cx, |preview, _| {
            preview.rows.len() == 3
                && preview.rows[1].depth == 1
                && preview.tree_children.len() == 2
                && !preview
                    .tree_children
                    .contains_key(&std::path::PathBuf::from("src/nested"))
        }));
        preview.update(cx, |preview, cx| preview.toggle_directory("src".into(), cx));
        assert_eq!(preview.read_with(cx, |preview, _| preview.rows.len()), 1);
        std::fs::write(root.join("src/new.rs"), "new").unwrap();
        preview.update(cx, |preview, cx| preview.toggle_directory("src".into(), cx));
        cx.run_until_parked();
        assert_eq!(preview.read_with(cx, |preview, _| preview.rows.len()), 3);
        preview.update(cx, |preview, cx| {
            preview.toggle_directory("src/nested".into(), cx);
            // Collapsing a parent while its child loads must not reveal descendants.
            preview.toggle_directory("src".into(), cx);
        });
        cx.run_until_parked();
        assert_eq!(preview.read_with(cx, |preview, _| preview.rows.len()), 1);
        preview.update(cx, |preview, cx| preview.toggle_directory("src".into(), cx));
        assert_eq!(preview.read_with(cx, |preview, _| preview.rows.len()), 4);
        preview.update(cx, |preview, cx| {
            preview.tree_children.clear();
            preview.load_directory("src".into(), cx);
            // Simulate Refresh invalidating an in-flight directory request.
            preview.tree_generation += 1;
            preview.tree_loading.clear();
            preview.tree_expanded.clear();
            preview.load_list(cx);
        });
        cx.run_until_parked();
        assert!(preview.read_with(cx, |preview, _| preview.rows.len() == 1
            && preview.tree_children.len() == 1));
        preview.update(cx, |preview, cx| preview.toggle_directory("src".into(), cx));
        cx.run_until_parked();
        assert_eq!(preview.read_with(cx, |preview, _| preview.rows.len()), 4);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn split_diff_pairs_replacements_without_crossing_hunks() {
        let rows = parse_diff("@@ -1,2 +1,1 @@\n-old1\n-old2\n+new1\n@@ -9 +8 @@\n context");
        assert_eq!(
            super::pair_diff_rows(&rows),
            vec![
                (Some(0), Some(0)),
                (Some(1), Some(3)),
                (Some(2), None),
                (Some(4), Some(4)),
                (Some(5), Some(5))
            ]
        );
    }

    #[test]
    fn files_and_both_diff_sides_highlight_modern_languages() {
        for (path, before, after) in [
            (
                "main.ts",
                "const count: number = 1;",
                "const count: number = 2;",
            ),
            ("main.kt", "val count = 1", "val count = 2"),
            ("main.swift", "let count = 1", "let count = 2"),
            ("Cargo.toml", "name = \"before\"", "name = \"after\""),
        ] {
            let mut file = vec![super::CodeRow {
                new: Some(1),
                text: after.into(),
                kind: ' ',
                ..Default::default()
            }];
            super::color_code(std::path::Path::new(path), &mut file, false);
            assert!(!file[0].syntax.is_empty(), "file: {path}");
            let mut diff = parse_diff(&format!("@@ -1 +1 @@\n-{before}\n+{after}"));
            super::color_code(std::path::Path::new(path), &mut diff, true);
            assert!(!diff[1].old_syntax.is_empty(), "old side: {path}");
            assert!(!diff[1].syntax.is_empty(), "unified deletion: {path}");
            assert!(!diff[2].syntax.is_empty(), "new side: {path}");
        }
    }

    #[test]
    fn diff_highlighting_preserves_each_sides_comment_state() {
        let mut rows =
            parse_diff("@@ -1,2 +1,2 @@\n-/* old comment\n+let value = 1;\n shared_call();");
        super::color_code(std::path::Path::new("example.rs"), &mut rows, true);
        let context = rows.last().unwrap();
        assert!(
            context
                .old_syntax
                .iter()
                .any(|(_, kind)| *kind == super::TokenKind::Comment)
        );
        assert!(
            !context
                .syntax
                .iter()
                .any(|(_, kind)| *kind == super::TokenKind::Comment)
        );
        for row in &rows {
            let text = row.text.get(1..).unwrap_or_default();
            assert!(
                row.syntax
                    .iter()
                    .chain(&row.old_syntax)
                    .all(|(range, _)| text.get(range.clone()).is_some())
            );
        }
    }

    #[test]
    fn descendants_of_hidden_directories_stay_muted() {
        use std::path::Path;
        assert!(super::hidden_tree_path(Path::new(".cache/sub/deep"), true));
        assert!(super::hidden_tree_path(
            Path::new(".cache/sub/deep/file.rs"),
            false
        ));
        assert!(super::hidden_tree_path(
            Path::new("src/.config/nested/file.rs"),
            false
        ));
        assert!(!super::hidden_tree_path(
            Path::new("src/config/file.rs"),
            false
        ));
    }

    #[test]
    fn change_tree_keeps_scopes_stats_and_collapsed_directories_separate() {
        use super::{ChangeScope, FileRow, change_tree_rows};
        let file = |path: &str, scope, stats| FileRow {
            header: false,
            depth: 0,
            path: path.into(),
            directory: false,
            ignored: false,
            scope: Some(scope),
            status: "M".into(),
            stats,
        };
        let files = vec![
            file("crates/app/src/main.rs", ChangeScope::Staged, Some((3, 2))),
            file(
                "crates/app/src/main.rs",
                ChangeScope::Unstaged,
                Some((5, 1)),
            ),
            file("new.rs", ChangeScope::Untracked, None),
        ];
        let mut collapsed = std::collections::HashSet::new();
        let rows = change_tree_rows(&files, &collapsed);
        assert_eq!(rows.iter().filter(|row| row.header).count(), 3);
        assert_eq!(
            rows.iter()
                .find(|row| row.header && row.scope == Some(ChangeScope::Staged))
                .unwrap()
                .stats,
            Some((3, 2))
        );
        assert!(
            rows.iter()
                .any(|row| row.directory && row.path == std::path::Path::new("crates/app/src"))
        );
        collapsed.insert((ChangeScope::Staged, "crates".into()));
        let rows = change_tree_rows(&files, &collapsed);
        assert!(
            !rows
                .iter()
                .any(|row| !row.header && !row.directory && row.scope == Some(ChangeScope::Staged))
        );
        assert!(
            rows.iter().any(|row| !row.header
                && !row.directory
                && row.scope == Some(ChangeScope::Unstaged))
        );
        assert!(
            rows.iter()
                .any(|row| row.path == std::path::Path::new("new.rs"))
        );
    }

    #[test]
    fn multiple_hunks_reset_numbers_and_empty_ranges_are_valid() {
        let rows = parse_diff("@@ -0,0 +1,2 @@\n+one\n+two\n@@ -8,2 +10,1 @@\n-removed\n kept");
        assert_eq!((rows[1].old, rows[1].new), (None, Some(1)));
        assert_eq!((rows[2].old, rows[2].new), (None, Some(2)));
        assert_eq!((rows[4].old, rows[4].new), (Some(8), None));
        assert_eq!((rows[5].old, rows[5].new), (Some(9), Some(10)));
    }

    #[test]
    fn diff_numbers_follow_hunks_and_do_not_number_headers() {
        let rows = parse_diff(
            "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -3,2 +7,2 @@\n same\n-old\n+new\n\\ No newline at end of file",
        );
        assert_eq!(rows[2].new, None);
        assert_eq!((rows[4].old, rows[4].new), (Some(3), Some(7)));
        assert_eq!((rows[5].old, rows[5].new), (Some(4), None));
        assert_eq!((rows[6].old, rows[6].new), (None, Some(8)));
        assert_eq!(rows[7].new, None);
    }
}
