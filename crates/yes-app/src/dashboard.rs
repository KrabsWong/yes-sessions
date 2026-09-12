use std::{path::PathBuf, sync::Arc};

use chrono::{Days, Local, NaiveDate};
use gpui_kit::base::{Selectable as _, StyledExt};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    calendar::{Calendar, CalendarState, Date},
    chart::LineChart,
    menu::{DropdownMenu as _, PopupMenuItem},
    plot::{
        AXIS_GAP, IntoPlot, Plot,
        tooltip::{CrossLine, Dot, Tooltip, TooltipState},
    },
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use yes_core::usage::{
    UsageCache, UsageDataset, UsageDimension, UsageFilter, UsageReport, UsageTotals,
};
use yes_core::{AppType, Language, ProviderRegistry};

use crate::i18n::tr;

pub(crate) enum DashboardEvent {
    OpenSession(AppType, String),
    Close,
}

pub(crate) struct Dashboard {
    language: Language,
    registry: Arc<ProviderRegistry>,
    cache: Option<UsageCache>,
    cache_path: Option<PathBuf>,
    cache_loaded: bool,
    has_snapshot: bool,
    cache_save_failed: bool,
    dataset: UsageDataset,
    report: UsageReport,
    projects: Vec<String>,
    models: Vec<(AppType, String)>,
    filter: UsageFilter,
    dimension: UsageDimension,
    loading: bool,
    custom: bool,
    custom_open: bool,
    range_days: Option<u64>,
    calendars: [Entity<CalendarState>; 2],
    visible_groups: usize,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<DashboardEvent> for Dashboard {}

fn number(value: u64, known: usize) -> String {
    if known == 0 {
        return "···".into();
    }
    let digits = value.to_string();
    let mut result = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(digit);
    }
    result
}

// Keep the native chart's hover positioning, but format token counts as integers.
#[derive(IntoPlot)]
struct UsageTrend {
    chart: LineChart<(String, u64), String, f64>,
    data: Vec<(String, u64)>,
    language: Language,
}

impl Plot for UsageTrend {
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        Plot::paint(&mut self.chart, bounds, window, cx);
    }

    fn id(&self) -> Option<ElementId> {
        Plot::id(&self.chart)
    }

    fn tooltip_state(
        &self,
        position: Point<Pixels>,
        bounds: Bounds<Pixels>,
        cx: &App,
    ) -> Option<TooltipState> {
        self.chart.tooltip_state(position, bounds, cx)
    }

    fn tooltip(
        &self,
        state: &TooltipState,
        cursor: Point<Pixels>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let (date, value) = self.data.get(state.index)?;
        let stroke = cx.theme().primary;
        Some(
            Tooltip::new(cursor, bounds.size)
                .gap(px(8.))
                .cross_line(
                    CrossLine::new(state.cross_line).height(bounds.size.height.as_f32() - AXIS_GAP),
                )
                .dots(
                    state
                        .dots
                        .iter()
                        .map(|point| Dot::new(*point).stroke(cx.theme().background).fill(stroke)),
                )
                .title(date.clone())
                .row(stroke, tr(self.language, "usage.total"), number(*value, 1))
                .into_any_element(),
        )
    }
}

fn day_range(days: u64, today: NaiveDate) -> (Option<NaiveDate>, Option<NaiveDate>) {
    (
        today.checked_sub_days(Days::new(days.saturating_sub(1))),
        Some(today),
    )
}

impl Dashboard {
    pub(crate) fn new(
        registry: Arc<ProviderRegistry>,
        language: Language,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        gpui_kit::component::set_locale(if language == Language::Zh {
            "zh-CN"
        } else {
            "en"
        });
        let (from, until) = day_range(30, Local::now().date_naive());
        let filter = UsageFilter {
            from,
            until,
            ..Default::default()
        };
        let dataset = UsageDataset::default();
        let report = dataset.aggregate(&filter, UsageDimension::Tool);
        let calendars = [from, until].map(|date| {
            cx.new(|cx| {
                let mut calendar = CalendarState::new(window, cx);
                calendar.set_date(Date::Single(date), window, cx);
                calendar
            })
        });
        let subscriptions = calendars
            .iter()
            .map(|calendar| {
                cx.observe(calendar, |this, _, cx| {
                    if this.custom {
                        let from = this.calendars[0].read(cx).date().start();
                        let until = this.calendars[1].read(cx).date().start();
                        if (from, until) != (this.filter.from, this.filter.until) {
                            this.filter.from = from;
                            this.filter.until = until;
                            this.recompute(cx);
                        }
                    }
                })
            })
            .collect();
        Self {
            language,
            registry,
            cache: Some(UsageCache::default()),
            cache_path: UsageCache::default_path(),
            cache_loaded: false,
            has_snapshot: false,
            cache_save_failed: false,
            dataset,
            report,
            projects: Vec::new(),
            models: Vec::new(),
            filter,
            dimension: UsageDimension::Tool,
            loading: false,
            custom: false,
            custom_open: false,
            range_days: Some(30),
            calendars,
            visible_groups: 50,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        if self.language != language {
            gpui_kit::component::set_locale(if language == Language::Zh {
                "zh-CN"
            } else {
                "en"
            });
            self.language = language;
            cx.notify();
        }
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(mut cache) = self.cache.take() else {
            return;
        };
        self.loading = true;
        let load_cache = !self.cache_loaded;
        let cache_path = self.cache_path.clone();
        let registry = self.registry.clone();
        cx.spawn(async move |this, cx| {
            if load_cache {
                let path = cache_path.clone();
                let restored = cx
                    .background_executor()
                    .spawn(async move {
                        let cache = path.as_deref().and_then(UsageCache::load)?;
                        let dataset = cache.snapshot();
                        let projects = dataset.projects();
                        let models = dataset.models();
                        Some((cache, dataset, projects, models))
                    })
                    .await;
                let snapshot = restored.map(|(restored, dataset, projects, models)| {
                    cache = restored;
                    (dataset, projects, models)
                });
                if this
                    .update(cx, |this, cx| {
                        this.cache_loaded = true;
                        if let Some((dataset, projects, models)) = snapshot {
                            this.dataset = dataset;
                            this.projects = projects;
                            this.models = models;
                            this.has_snapshot = true;
                            this.recompute(cx);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let task = cx.background_executor().spawn(async move {
                let dataset = cache.refresh(&registry);
                let projects = dataset.projects();
                let models = dataset.models();
                let save_failed = cache_path
                    .as_deref()
                    .is_none_or(|path| cache.save(path).is_err());
                (cache, dataset, projects, models, save_failed)
            });
            let (cache, dataset, projects, models, save_failed) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.cache = Some(cache);
                this.dataset = dataset;
                this.projects = projects;
                this.models = models;
                this.loading = false;
                this.has_snapshot = true;
                this.cache_save_failed = save_failed;
                if !this.custom
                    && let Some(days) = this.range_days
                {
                    (this.filter.from, this.filter.until) =
                        day_range(days, Local::now().date_naive());
                }
                this.recompute(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn refresh_label(&self) -> &'static str {
        if !self.loading {
            "dashboard.refresh"
        } else if !self.cache_loaded {
            "dashboard.readingCache"
        } else if self.has_snapshot {
            "dashboard.updating"
        } else {
            "dashboard.loading"
        }
    }

    fn recompute(&mut self, cx: &mut Context<Self>) {
        self.report = self.dataset.aggregate(&self.filter, self.dimension);
        self.visible_groups = 50;
        cx.notify();
    }

    fn choose_range(&mut self, days: Option<u64>, cx: &mut Context<Self>) {
        self.custom = false;
        self.custom_open = false;
        self.range_days = days;
        (self.filter.from, self.filter.until) = days
            .map(|days| day_range(days, Local::now().date_naive()))
            .unwrap_or((None, None));
        self.recompute(cx);
    }

    fn set_tool_filter(&mut self, app: Option<AppType>) {
        if self.filter.app_type != app {
            self.filter.model = None;
        }
        self.filter.app_type = app;
    }

    fn set_model_filter(&mut self, model: Option<String>, app: Option<AppType>) {
        if model.is_some() {
            self.filter.app_type = app;
        }
        self.filter.model = model;
    }

    fn model_label(&self, model: &str, app: Option<AppType>) -> String {
        let model = if model.is_empty() {
            tr(self.language, "dashboard.unknown")
        } else {
            model
        };
        let agent = app
            .map(|app| app.display_name())
            .unwrap_or_else(|| tr(self.language, "dashboard.tools"));
        format!("{model} · {agent}")
    }

    fn filter_menu(&self, dimension: UsageDimension, cx: &mut Context<Self>) -> impl IntoElement {
        let (id, title, selected, options) = match dimension {
            UsageDimension::Tool => (
                "dashboard-tool",
                "dashboard.tools",
                self.filter
                    .app_type
                    .map(|app| app.display_name().to_owned()),
                AppType::ALL
                    .iter()
                    .map(|app| (app.as_str().to_owned(), app.display_name().to_owned(), None))
                    .collect::<Vec<_>>(),
            ),
            UsageDimension::Project => (
                "dashboard-project",
                "dashboard.projects",
                self.filter.project.clone(),
                self.projects
                    .iter()
                    .cloned()
                    .map(|value| (value.clone(), value, None))
                    .collect(),
            ),
            _ => (
                "dashboard-model",
                "dashboard.models",
                self.filter
                    .model
                    .as_deref()
                    .map(|model| self.model_label(model, self.filter.app_type)),
                self.models
                    .iter()
                    .map(|(app, model)| {
                        (
                            model.clone(),
                            self.model_label(model, Some(*app)),
                            Some(*app),
                        )
                    })
                    .collect(),
            ),
        };
        let lang = self.language;
        let label = selected
            .map(|s| {
                if s.is_empty() {
                    tr(lang, "dashboard.unknown").to_owned()
                } else {
                    s
                }
            })
            .unwrap_or_else(|| tr(lang, title).to_owned());
        let owner = cx.weak_entity();
        Button::new(id)
            .outline()
            .small()
            .max_w(px(260.))
            .tooltip(label.clone())
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                std::iter::once((None, tr(lang, title).to_owned(), None))
                    .chain(options.iter().map(|(key, label, app)| {
                        (
                            Some(key.clone()),
                            if label.is_empty() {
                                tr(lang, "dashboard.unknown").into()
                            } else {
                                label.clone()
                            },
                            *app,
                        )
                    }))
                    .fold(
                        menu.scrollable(true).max_h(px(360.)),
                        |menu, (key, label, app)| {
                            let owner = owner.clone();
                            menu.item(
                                PopupMenuItem::element(move |_, _| {
                                    div().max_w(px(500.)).overflow_hidden().child(label.clone())
                                })
                                .on_click(move |_, _, cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        match dimension {
                                            UsageDimension::Tool => this.set_tool_filter(
                                                key.as_deref().and_then(|key| key.parse().ok()),
                                            ),
                                            UsageDimension::Project => {
                                                this.filter.project = key.clone()
                                            }
                                            _ => this.set_model_filter(key.clone(), app),
                                        }
                                        this.recompute(cx);
                                    });
                                }),
                            )
                        },
                    )
            })
    }

    fn metric(&self, label: &'static str, value: String, cx: &App) -> impl IntoElement {
        div()
            .v_flex()
            .gap_2()
            .min_w(px(150.))
            .flex_1()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(self.language, label)),
            )
            .child(
                div()
                    .text_size(px(24.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(value),
            )
    }

    fn trend(&self, cx: &App) -> AnyElement {
        let mut data = Vec::new();
        let from = self
            .filter
            .from
            .or_else(|| self.report.daily.first().map(|(day, _)| *day));
        let until = self
            .filter
            .until
            .or_else(|| self.report.daily.last().map(|(day, _)| *day));
        if let (Some(mut day), Some(until)) = (from, until) {
            let known = self
                .report
                .daily
                .iter()
                .map(|(day, totals)| (*day, totals.total_tokens))
                .collect::<std::collections::HashMap<_, _>>();
            // Bound the chart to the supported chrono date span without allocating a point for each day of an arbitrary range.
            let span = (until - day).num_days().max(0) as u64 + 1;
            let stride = span.div_ceil(366).max(1);
            while day <= until {
                let end = day
                    .checked_add_days(Days::new(stride - 1))
                    .unwrap_or(until)
                    .min(until);
                let value = known
                    .iter()
                    .filter(|(date, _)| **date >= day && **date <= end)
                    .map(|(_, value)| *value)
                    .fold(0u64, u64::saturating_add);
                data.push((
                    if stride == 1 {
                        day.to_string()
                    } else {
                        format!("{day} / {end}")
                    },
                    value,
                ));
                let Some(next) = end.succ_opt() else { break };
                day = next;
            }
        }
        if data.is_empty() || self.report.totals.total_records == 0 {
            return div()
                .h(px(150.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(tr(self.language, "dashboard.noUsage"))
                .into_any_element();
        }
        let tick_margin = data.len().div_ceil(6).max(1);
        div()
            .h(px(190.))
            .w_full()
            .child(UsageTrend {
                chart: LineChart::new(data.clone())
                    .id("dashboard-trend-chart")
                    .x(|item| item.0.clone())
                    .y(|item| item.1 as f64)
                    .stroke(cx.theme().primary)
                    .linear()
                    .dot()
                    .tick_margin(tick_margin),
                data,
                language: self.language,
            })
            .into_any_element()
    }
}

impl Render for Dashboard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.language;
        let totals = &self.report.totals;
        let invalid_range = matches!((self.filter.from, self.filter.until), (Some(from), Some(until)) if from > until);
        let incomplete = totals.incomplete()
            || self.dataset.failed_sessions > 0
            || self.dataset.failed_providers > 0;
        let header = div()
            .h_flex()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr(lang, "dashboard.title")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(tr(lang, "dashboard.basis")),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("dashboard-refresh")
                            .outline()
                            .small()
                            .label(tr(lang, self.refresh_label()))
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .child(
                        Button::new("dashboard-close")
                            .ghost()
                            .small()
                            .label(tr(lang, "dashboard.back"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DashboardEvent::Close))),
                    ),
            );
        let periods = div()
            .h_flex()
            .flex_wrap()
            .gap_2()
            .children(
                [
                    (Some(1), "timeRange.today"),
                    (Some(7), "dashboard.week"),
                    (Some(30), "timeRange.last30Days"),
                    (None, "dashboard.allTime"),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (days, key))| {
                    Button::new(("dashboard-period", index))
                        .debug_selector(move || format!("dashboard-period-{index}").into())
                        .small()
                        .ghost()
                        .selected(!self.custom && self.range_days == days)
                        .label(tr(lang, key))
                        .on_click(cx.listener(move |this, _, _, cx| this.choose_range(days, cx)))
                }),
            )
            .child(
                Button::new("dashboard-custom")
                    .debug_selector(|| "dashboard-custom".into())
                    .small()
                    .ghost()
                    .label(tr(lang, "timeRange.custom"))
                    .selected(self.custom)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if !this.custom {
                            this.custom = true;
                            for (calendar, date) in this
                                .calendars
                                .iter()
                                .zip([this.filter.from, this.filter.until])
                            {
                                calendar.update(cx, |state, cx| {
                                    state.set_date(Date::Single(date), window, cx)
                                });
                            }
                        }
                        this.custom_open = !this.custom_open;
                        cx.notify();
                    })),
            );
        let filters = div()
            .h_flex()
            .flex_wrap()
            .gap_2()
            .child(self.filter_menu(UsageDimension::Tool, cx))
            .child(self.filter_menu(UsageDimension::Project, cx))
            .child(self.filter_menu(UsageDimension::Model, cx))
            .child(
                Button::new("dashboard-clear")
                    .small()
                    .ghost()
                    .label(tr(lang, "dashboard.clear"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filter.app_type = None;
                        this.filter.project = None;
                        this.filter.model = None;
                        this.recompute(cx);
                    })),
            );
        let metrics = div()
            .h_flex()
            .flex_wrap()
            .gap_4()
            .py_4()
            .border_y_1()
            .border_color(cx.theme().border)
            .child(self.metric(
                "usage.total",
                number(totals.total_tokens, totals.total_records),
                cx,
            ))
            .child(self.metric(
                "usage.input",
                number(totals.input_tokens, totals.input_records),
                cx,
            ))
            .child(self.metric(
                "usage.output",
                number(totals.output_tokens, totals.output_records),
                cx,
            ))
            .child(self.metric(
                "usage.cached",
                number(totals.cache_read_tokens, totals.cache_records),
                cx,
            ))
            .child(
                self.metric(
                    "usage.hitRate",
                    totals
                        .cache_hit_rate()
                        .map(|rate| format!("{rate:.1}%"))
                        .unwrap_or_else(|| "···".into()),
                    cx,
                ),
            );
        let dimensions = [
            (UsageDimension::Tool, "dashboard.byTool"),
            (UsageDimension::Project, "dashboard.byProject"),
            (UsageDimension::Model, "dashboard.byModel"),
            (UsageDimension::Session, "dashboard.sessions"),
            (UsageDimension::Kind, "dashboard.agents"),
        ];
        let tabs =
            div()
                .h_flex()
                .flex_wrap()
                .gap_2()
                .children(
                    dimensions
                        .into_iter()
                        .enumerate()
                        .map(|(index, (dimension, key))| {
                            Button::new(("dashboard-dimension", index))
                                .debug_selector(move || {
                                    format!("dashboard-dimension-{index}").into()
                                })
                                .small()
                                .ghost()
                                .selected(self.dimension == dimension)
                                .label(tr(lang, key))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.dimension = dimension;
                                    this.recompute(cx);
                                }))
                        }),
                );
        let table_header = div()
            .h_flex()
            .gap_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(div().flex_1().child(tr(lang, "dashboard.group")))
            .children(
                [
                    "usage.input",
                    "usage.output",
                    "usage.total",
                    "usage.hitRate",
                ]
                .map(|key| div().w(px(120.)).text_right().child(tr(lang, key))),
            );
        let rows = self
            .report
            .groups
            .iter()
            .take(self.visible_groups)
            .enumerate()
            .map(|(index, group)| {
                let label = if group.label.is_empty() {
                    tr(lang, "dashboard.unknown").to_owned()
                } else if self.dimension == UsageDimension::Kind {
                    tr(
                        lang,
                        if group.key == "main" {
                            "dashboard.main"
                        } else {
                            "dashboard.subagent"
                        },
                    )
                    .to_owned()
                } else {
                    group.label.clone()
                };
                let app_type = group.app_type;
                let session_id = group.session_id.clone();
                let key = group.key.clone();
                let dimension = self.dimension;
                let values: &UsageTotals = &group.totals;
                let share = values.total_tokens as f64 / totals.total_tokens.max(1) as f64;
                div()
                    .h_flex()
                    .gap_4()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.5))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(180.))
                            .v_flex()
                            .gap_1()
                            .child(
                                Button::new(("dashboard-row", index))
                                    .debug_selector(move || format!("dashboard-row-{index}").into())
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .tooltip(label.clone())
                                    .child(
                                        div()
                                            .w_full()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(label),
                                    )
                                    .disabled(dimension == UsageDimension::Kind)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        match dimension {
                                            UsageDimension::Session => {
                                                if let (Some(app), Some(id)) =
                                                    (app_type, session_id.clone())
                                                {
                                                    cx.emit(DashboardEvent::OpenSession(app, id));
                                                }
                                            }
                                            UsageDimension::Tool => {
                                                this.set_tool_filter(key.parse().ok())
                                            }
                                            UsageDimension::Project => {
                                                this.filter.project = Some(key.clone())
                                            }
                                            UsageDimension::Model => {
                                                this.filter.model = Some(key.clone())
                                            }
                                            UsageDimension::Kind => {}
                                        }
                                        this.recompute(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .h_flex()
                                    .gap_2()
                                    .child(
                                        div().flex_1().h(px(2.)).bg(cx.theme().muted).child(
                                            div()
                                                .h_full()
                                                .w(relative(share as f32))
                                                .bg(cx.theme().primary.opacity(0.6)),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(
                                                if values.total_records > 0
                                                    && totals.total_tokens > 0
                                                {
                                                    format!("{:.1}%", share * 100.)
                                                } else {
                                                    "···".into()
                                                },
                                            ),
                                    ),
                            ),
                    )
                    .children(
                        [
                            number(values.input_tokens, values.input_records),
                            number(values.output_tokens, values.output_records),
                            number(values.total_tokens, values.total_records),
                            values
                                .cache_hit_rate()
                                .map(|rate| format!("{rate:.1}%"))
                                .unwrap_or_else(|| "···".into()),
                        ]
                        .map(|value| {
                            div()
                                .w(px(120.))
                                .flex_none()
                                .text_right()
                                .text_sm()
                                .child(value)
                        }),
                    )
            })
            .collect::<Vec<_>>();
        let content = div()
            .w_full()
            .flex_none()
            .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
            .v_flex()
            .gap_4()
            .p_4()
            .child(header)
            .child(periods)
            .when(self.custom_open, |view| {
                view.child(div().h_flex().flex_wrap().gap_4().children(
                    self.calendars.iter().enumerate().map(|(i, calendar)| {
                        div()
                            .v_flex()
                            .gap_2()
                            .child(tr(
                                lang,
                                if i == 0 {
                                    "timeRange.from"
                                } else {
                                    "timeRange.until"
                                },
                            ))
                            .child(Calendar::new(calendar).small())
                    }),
                ))
            })
            .child(filters)
            .when(invalid_range, |view| {
                view.child(
                    div()
                        .text_color(cx.theme().danger)
                        .child(tr(lang, "timeRange.invalidOrder")),
                )
            })
            .child(metrics)
            .when(self.has_snapshot && self.loading, |view| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(tr(lang, "dashboard.cachedSnapshot")),
                )
            })
            .when(self.cache_save_failed, |view| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(tr(lang, "dashboard.cacheSaveFailed")),
                )
            })
            .when(incomplete, |view| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} · {}/{} · {} {} / {} {}",
                            tr(lang, "dashboard.incomplete"),
                            totals.usage_records,
                            totals.records,
                            self.dataset.failed_sessions,
                            tr(lang, "dashboard.failedSessions"),
                            self.dataset.failed_providers,
                            tr(lang, "dashboard.failedProviders")
                        )),
                )
            })
            .when(self.report.unknown_date_records > 0, |view| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{}: {}",
                            tr(lang, "dashboard.unknownDates"),
                            self.report.unknown_date_records
                        )),
                )
            })
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(tr(lang, "dashboard.trend")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(tr(lang, "dashboard.trendHint")),
                    ),
            )
            .child(self.trend(cx))
            .child(tabs)
            .when(self.report.groups.is_empty() && !self.loading, |view| {
                view.child(
                    div()
                        .py_4()
                        .text_color(cx.theme().muted_foreground)
                        .child(tr(lang, "dashboard.empty")),
                )
            })
            .child(
                div().id("dashboard-table").overflow_x_scroll().child(
                    div()
                        .v_flex()
                        .min_w(px(760.))
                        .child(table_header)
                        .children(rows),
                ),
            )
            .when(self.report.groups.len() > self.visible_groups, |view| {
                view.child(
                    Button::new("dashboard-more")
                        .ghost()
                        .label(tr(lang, "dashboard.more"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.visible_groups += 50;
                            cx.notify();
                        })),
                )
            });
        div()
            .id("usage-dashboard")
            .debug_selector(|| "usage-dashboard".into())
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .overflow_y_scroll()
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::{Dashboard, DashboardEvent, day_range, number};
    use chrono::{Local, NaiveDate};
    use gpui_kit::{TestAppContext, VisualTestContext, px, size};
    use std::sync::Arc;
    use yes_core::{AppType, Language, ProviderRegistry, usage::UsageDimension};
    use yes_core::{
        model::{SessionKind, TokenUsage},
        usage::UsageRecord,
    };

    #[test]
    fn date_presets_and_unknown_numbers_are_explicit() {
        let today = NaiveDate::from_ymd_opt(2026, 1, 3).unwrap();
        assert_eq!(day_range(1, today), (Some(today), Some(today)));
        assert_eq!(day_range(7, today).0, NaiveDate::from_ymd_opt(2025, 12, 28));
        assert_eq!(number(0, 0), "···");
        assert_eq!(number(0, 1), "0");
        assert_eq!(number(1234567, 1), "1,234,567");
        assert_eq!(number(325078092, 1), "325,078,092");
        assert_eq!(number(9_007_199_254_740_993, 1), "9,007,199,254,740,993");
    }

    #[gpui_kit::test]
    fn restart_restores_usage_before_refresh_and_write_failure_keeps_results(
        cx: &mut TestAppContext,
    ) {
        use std::{
            cell::Cell,
            rc::Rc,
            sync::atomic::{AtomicUsize, Ordering},
            time::{SystemTime, UNIX_EPOCH},
        };
        use yes_core::{
            MessageType, Session, SessionDetail, SessionMessage, SessionProvider, usage::UsageCache,
        };
        struct Provider {
            app: AppType,
            detail: SessionDetail,
            reads: Arc<AtomicUsize>,
        }
        impl SessionProvider for Provider {
            fn app_type(&self) -> AppType {
                self.app
            }
            fn is_available(&self) -> bool {
                self.app == AppType::Codex
            }
            fn sessions(&self) -> anyhow::Result<Vec<Session>> {
                Ok(vec![self.detail.session.clone()])
            }
            fn session_detail(&self, _: &str) -> anyhow::Result<Option<SessionDetail>> {
                self.reads.fetch_add(1, Ordering::SeqCst);
                Ok(Some(self.detail.clone()))
            }
        }
        let root = std::env::temp_dir().join(format!(
            "yes-dashboard-cache-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("session.jsonl");
        std::fs::write(&source, "source").unwrap();
        let mut message =
            SessionMessage::text(MessageType::Assistant, Local::now().to_rfc3339(), "reply");
        message.usage = Some(TokenUsage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            total_tokens: Some(120),
            cache_read_tokens: Some(50),
        });
        let detail = SessionDetail {
            subtree_usage: None,
            session: Session {
                id: "cached-session".into(),
                app_type: AppType::Codex,
                file_name: "session.jsonl".into(),
                file_path: source,
                created_at: 0,
                updated_at: 0,
                message_count: 1,
                first_message: "Cached session".into(),
                last_message: String::new(),
                directory: None,
                uuid: None,
                kind: SessionKind::Main,
                parent_session_id: None,
                agent_type: None,
            },
            messages: vec![message],
        };
        let reads = Arc::new(AtomicUsize::new(0));
        let mut registry = ProviderRegistry::default();
        for app in AppType::ALL {
            registry.register(Arc::new(Provider {
                app,
                detail: detail.clone(),
                reads: reads.clone(),
            }));
        }
        let mut cache = UsageCache::default();
        cache.refresh(&registry);
        let path = root.join("usage.json");
        cache.save(&path).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1000.), px(900.)), |window, cx| {
            Dashboard::new(Arc::new(registry), Language::En, window, cx)
        });
        let dashboard = window.root(cx).unwrap();
        let saw_cached = Rc::new(Cell::new(false));
        let observed = saw_cached.clone();
        let _subscription = cx.update(|cx| {
            cx.observe(&dashboard, move |dashboard, cx| {
                let dashboard = dashboard.read(cx);
                if dashboard.loading
                    && dashboard.has_snapshot
                    && dashboard.report.totals.total_tokens == 120
                {
                    assert_eq!(dashboard.refresh_label(), "dashboard.updating");
                    observed.set(true);
                }
            })
        });
        dashboard.update(cx, |dashboard, cx| {
            dashboard.cache_path = Some(path);
            dashboard.refresh(cx);
            assert_eq!(dashboard.refresh_label(), "dashboard.readingCache");
        });
        cx.run_until_parked();
        assert!(saw_cached.get());
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "unchanged source must not be reparsed after restart"
        );
        dashboard.read_with(cx, |dashboard, _| {
            assert!(!dashboard.loading);
            assert!(dashboard.has_snapshot);
            assert!(!dashboard.cache_save_failed);
            assert_eq!(dashboard.report.totals.total_tokens, 120);
            assert_eq!(dashboard.refresh_label(), "dashboard.refresh");
        });
        dashboard.update(cx, |dashboard, cx| {
            dashboard.cache_path = Some(root.clone()); // A directory cannot be replaced by the cache file.
            dashboard.refresh(cx);
            assert_eq!(dashboard.refresh_label(), "dashboard.updating");
        });
        cx.run_until_parked();
        dashboard.read_with(cx, |dashboard, _| {
            assert!(dashboard.cache_save_failed);
            assert_eq!(dashboard.report.totals.total_tokens, 120);
        });
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui_kit::test]
    fn model_selection_tracks_its_agent_and_tool_changes_clear_it(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1000.), px(900.)), |window, cx| {
            Dashboard::new(
                Arc::new(ProviderRegistry::default()),
                Language::En,
                window,
                cx,
            )
        });
        let dashboard = window.root(cx).unwrap();
        dashboard.update(cx, |dashboard, _| {
            dashboard.filter.project = Some("/work/project".into());
            dashboard.set_model_filter(Some("shared-model".into()), Some(AppType::Codex));
            assert_eq!(dashboard.filter.app_type, Some(AppType::Codex));
            assert_eq!(
                dashboard.model_label("shared-model", dashboard.filter.app_type),
                "shared-model · Codex CLI"
            );
            dashboard.set_model_filter(Some("shared-model".into()), Some(AppType::Claude));
            assert_eq!(dashboard.filter.app_type, Some(AppType::Claude));
            dashboard.set_model_filter(None, None);
            assert_eq!(dashboard.filter.app_type, Some(AppType::Claude));
            dashboard.set_model_filter(Some("shared-model".into()), Some(AppType::Codex));
            dashboard.set_tool_filter(Some(AppType::Codex));
            assert_eq!(dashboard.filter.model.as_deref(), Some("shared-model"));
            dashboard.set_tool_filter(Some(AppType::Claude));
            assert!(dashboard.filter.model.is_none());
            assert_eq!(dashboard.filter.project.as_deref(), Some("/work/project"));
            assert_eq!(
                dashboard.model_label("", Some(AppType::Codex)),
                "Unknown · Codex CLI"
            );
        });
    }

    #[gpui_kit::test]
    fn dashboard_filters_render_and_drill_down(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1000.), px(900.)), |window, cx| {
            Dashboard::new(
                Arc::new(ProviderRegistry::default()),
                Language::En,
                window,
                cx,
            )
        });
        let dashboard = window.root(cx).unwrap();
        dashboard.update(cx, |dashboard, cx| {
            let today = Local::now().date_naive();
            dashboard.dataset.records =
                [(today, "new", 125), (today.pred_opt().unwrap(), "old", 375)]
                    .map(|(date, id, total)| UsageRecord {
                        date: Some(date),
                        app_type: AppType::Codex,
                        project: Some("/work/dashboard".into()),
                        model: Some("test-model".into()),
                        session_id: id.into(),
                        session_title: id.into(),
                        kind: SessionKind::Main,
                        usage: Some(TokenUsage {
                            input_tokens: Some(total - 25),
                            output_tokens: Some(25),
                            total_tokens: Some(total),
                            cache_read_tokens: Some(50),
                        }),
                    })
                    .to_vec();
            dashboard.recompute(cx);
        });
        cx.run_until_parked();
        let mut visual = VisualTestContext::from_window(*window, cx);
        assert!(visual.debug_bounds("usage-dashboard").is_some());
        let today = visual.debug_bounds("dashboard-period-0").unwrap().center();
        visual.simulate_click(today, Default::default());
        cx.run_until_parked();
        dashboard.read_with(cx, |dashboard, _| {
            assert_eq!(dashboard.report.totals.total_tokens, 125)
        });
        let sessions = visual
            .debug_bounds("dashboard-dimension-3")
            .unwrap()
            .center();
        visual.simulate_click(sessions, Default::default());
        cx.run_until_parked();
        dashboard.read_with(cx, |dashboard, _| {
            assert_eq!(dashboard.dimension, UsageDimension::Session);
            assert_eq!(
                dashboard.report.groups[0].session_id.as_deref(),
                Some("new")
            );
        });
        let opened = std::rc::Rc::new(std::cell::RefCell::new(None));
        let captured = opened.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&dashboard, move |_, event, _| {
                if let DashboardEvent::OpenSession(app, id) = event {
                    *captured.borrow_mut() = Some((*app, id.clone()));
                }
            })
        });
        let row = visual.debug_bounds("dashboard-row-0").unwrap().center();
        visual.simulate_click(row, Default::default());
        cx.run_until_parked();
        assert_eq!(*opened.borrow(), Some((AppType::Codex, "new".into())));
        // Keeping a custom range selected while closing its calendar must survive refreshes.
        let custom = visual.debug_bounds("dashboard-custom").unwrap().center();
        visual.simulate_click(custom, Default::default());
        cx.run_until_parked();
        let custom = visual.debug_bounds("dashboard-custom").unwrap().center();
        visual.simulate_click(custom, Default::default());
        cx.run_until_parked();
        dashboard.read_with(cx, |dashboard, _| {
            assert!(dashboard.custom);
            assert!(!dashboard.custom_open);
        });
        dashboard.update(cx, |dashboard, cx| {
            dashboard.set_language(Language::Zh, cx);
        });
        cx.run_until_parked();
        assert!(visual.debug_bounds("dashboard-period-0").is_some());
    }
}
