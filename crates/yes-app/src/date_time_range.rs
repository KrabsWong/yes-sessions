use crate::i18n::tr;
use chrono::{DateTime, Duration, Local, NaiveDateTime, NaiveTime, TimeZone, Timelike};
use gpui_kit::base::{Disableable, Selectable, StyledExt};
use gpui_kit::component::{
    ActiveTheme, IndexPath, Sizable,
    button::{Button, ButtonVariants},
    calendar::{Calendar, CalendarState, Date},
    select::{Select, SelectEvent, SelectState},
};
use gpui_kit::*;
use yes_core::Language;

type TimeSelect = SelectState<Vec<String>>;

#[derive(Clone)]
pub(crate) enum DateTimeRangeEvent {
    Change,
}

pub(crate) struct DateTimeRangePicker {
    language: Language,
    calendars: [Entity<CalendarState>; 2],
    times: [[Entity<TimeSelect>; 3]; 2],
    preset: Option<usize>,
    pub(crate) custom_open: bool,
    last_range: [Option<NaiveDateTime>; 2],
    error: Option<&'static str>,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<DateTimeRangeEvent> for DateTimeRangePicker {}

const PRESETS: [(&str, i64); 11] = [
    ("timeRange.last1Minute", 60),
    ("timeRange.last5Minutes", 300),
    ("timeRange.last30Minutes", 1800),
    ("timeRange.last1Hour", 3600),
    ("timeRange.last12Hours", 43200),
    ("timeRange.today", 0),
    ("timeRange.yesterday", 0),
    ("timeRange.last3Days", 259200),
    ("timeRange.last1Week", 604800),
    ("timeRange.last15Days", 1296000),
    ("timeRange.last30Days", 2592000),
];

// Calendar days resolve independently in local time, preserving DST day lengths.
fn preset_range<T: TimeZone>(
    index: usize,
    now: DateTime<T>,
) -> Result<(DateTime<T>, DateTime<T>), &'static str> {
    let timezone = now.timezone();
    if index == 5 || index == 6 {
        let end_date = if index == 5 {
            now.date_naive().succ_opt()
        } else {
            Some(now.date_naive())
        }
        .ok_or("timeRange.invalidTime")?;
        let start_date = end_date.pred_opt().ok_or("timeRange.invalidTime")?;
        let resolve = |date: chrono::NaiveDate| {
            timezone
                .from_local_datetime(&date.and_time(NaiveTime::MIN))
                .single()
                .ok_or("timeRange.invalidTime")
        };
        return Ok((resolve(start_date)?, resolve(end_date)?));
    }
    let seconds = PRESETS.get(index).ok_or("timeRange.invalidTime")?.1;
    let end = now
        .with_nanosecond(0)
        .ok_or("timeRange.invalidTime")?
        .checked_add_signed(Duration::seconds(1))
        .ok_or("timeRange.invalidTime")?;
    let start = end
        .clone()
        .checked_sub_signed(Duration::seconds(seconds))
        .ok_or("timeRange.invalidTime")?;
    Ok((start, end))
}

fn resolve_bounds(
    from: Option<NaiveDateTime>,
    until: Option<NaiveDateTime>,
) -> Result<(Option<i64>, Option<i64>), &'static str> {
    let resolve = |value: NaiveDateTime| {
        Local
            .from_local_datetime(&value)
            .single()
            .map(|time| time.timestamp_millis())
            .ok_or("timeRange.invalidTime")
    };
    let from = from.map(resolve).transpose()?;
    let until = until
        .map(|time| {
            resolve(time)?
                .checked_add(1000)
                .ok_or("timeRange.invalidTime")
        })
        .transpose()?;
    if matches!((from, until), (Some(from), Some(until)) if from >= until) {
        return Err("timeRange.invalidOrder");
    }
    Ok((from, until))
}

impl DateTimeRangePicker {
    pub(crate) fn new(language: Language, window: &mut Window, cx: &mut Context<Self>) -> Self {
        gpui_kit::component::set_locale(if language == Language::Zh {
            "zh-CN"
        } else {
            "en"
        });
        let calendars = std::array::from_fn(|_| {
            cx.new(|cx| {
                let mut state = CalendarState::new(window, cx);
                state.set_date(Date::Single(None), window, cx);
                state
            })
        });
        let times = std::array::from_fn(|end| {
            std::array::from_fn(|part| {
                let count = if part == 0 { 24 } else { 60 };
                cx.new(|cx| {
                    SelectState::new(
                        (0..count).map(|n| format!("{n:02}")).collect(),
                        Some(IndexPath::new(if end == 0 { 0 } else { count - 1 })),
                        window,
                        cx,
                    )
                })
            })
        });
        let mut subscriptions = Vec::new();
        for calendar in &calendars {
            subscriptions.push(cx.observe(calendar, |this, _, cx| {
                this.selection_changed(cx);
            }));
        }
        for endpoint in &times {
            for time in endpoint {
                subscriptions.push(
                    cx.subscribe(time, |this, _, _: &SelectEvent<Vec<String>>, cx| {
                        this.selection_changed(cx)
                    }),
                );
            }
        }
        Self {
            language,
            calendars,
            times,
            preset: None,
            custom_open: false,
            last_range: [None, None],
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn selection_changed(&mut self, cx: &mut Context<Self>) {
        if self.last_range != [self.endpoint(0, cx), self.endpoint(1, cx)] {
            self.changed(cx);
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.last_range = [self.endpoint(0, cx), self.endpoint(1, cx)];
        self.preset = None;
        self.error = None;
        cx.emit(DateTimeRangeEvent::Change);
        cx.notify();
    }

    fn endpoint(&self, end: usize, cx: &App) -> Option<NaiveDateTime> {
        let date = self.calendars[end].read(cx).date().start()?;
        let time: Vec<u32> = self.times[end]
            .iter()
            .map(|state| {
                state
                    .read(cx)
                    .selected_value()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0)
            })
            .collect();
        date.and_hms_opt(time[0], time[1], time[2])
    }

    pub(crate) fn bounds(&self, cx: &App) -> Result<(Option<i64>, Option<i64>), &'static str> {
        if let Some(error) = self.error {
            return Err(error);
        }
        resolve_bounds(self.endpoint(0, cx), self.endpoint(1, cx))
    }

    pub(crate) fn summary(&self, cx: &App) -> String {
        if let Some(index) = self.preset {
            return tr(self.language, PRESETS[index].0).to_owned();
        }
        if !self.has_value(cx) {
            return tr(self.language, "agentSearch.unlimited").to_owned();
        }
        let endpoint = |end| {
            self.endpoint(end, cx)
                .map(|value| value.format("%m/%d").to_string())
                .unwrap_or_else(|| tr(self.language, "agentSearch.unlimited").to_owned())
        };
        format!("{} → {}", endpoint(0), endpoint(1))
    }

    pub(crate) fn description(&self, cx: &App) -> String {
        let endpoint = |end| {
            self.endpoint(end, cx)
                .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| tr(self.language, "agentSearch.unlimited").to_owned())
        };
        format!("{} → {}", endpoint(0), endpoint(1))
    }

    pub(crate) fn has_value(&self, cx: &App) -> bool {
        self.endpoint(0, cx).is_some() || self.endpoint(1, cx).is_some()
    }

    pub(crate) fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        if self.language == language {
            return;
        }
        self.language = language;
        gpui_kit::component::set_locale(if language == Language::Zh {
            "zh-CN"
        } else {
            "en"
        });
        cx.notify();
    }

    pub(crate) fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_range(None, None, window, cx);
    }

    /// The displayed end includes its entire second; bounds() returns the exclusive end.
    pub(crate) fn set_range(
        &mut self,
        from: Option<NaiveDateTime>,
        until: Option<NaiveDateTime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (end, value) in [from, until].into_iter().enumerate() {
            self.calendars[end].update(cx, |state, cx| {
                state.set_date(Date::Single(value.map(|v| v.date())), window, cx);
            });
            let time = value
                .map(|v| [v.hour(), v.minute(), v.second()])
                .unwrap_or(if end == 0 { [0, 0, 0] } else { [23, 59, 59] });
            for (part, value) in time.into_iter().enumerate() {
                self.times[end][part].update(cx, |state, cx| {
                    state.set_selected_index(Some(IndexPath::new(value as usize)), window, cx)
                });
            }
        }
        self.changed(cx);
    }

    fn apply_preset(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        match preset_range(index, Local::now()) {
            Ok((from, until)) => {
                self.set_range(
                    Some(from.naive_local()),
                    Some((until - Duration::seconds(1)).naive_local()),
                    window,
                    cx,
                );
                self.preset = Some(index);
            }
            Err(error) => {
                self.error = Some(error);
                cx.emit(DateTimeRangeEvent::Change);
            }
        }
        cx.notify();
    }
}

impl Render for DateTimeRangePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.language;
        let tabs = div()
            .h_flex()
            .w(px(264.))
            .gap_1()
            .p_1()
            .rounded_md()
            .bg(cx.theme().muted)
            .child(
                Button::new("time-back")
                    .small()
                    .ghost()
                    .flex_1()
                    .debug_selector(|| "time-range-back".into())
                    .label(tr(language, "timeRange.presets"))
                    .selected(!self.custom_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.custom_open = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("time-custom")
                    .small()
                    .ghost()
                    .flex_1()
                    .debug_selector(|| "time-custom".into())
                    .label(tr(language, "timeRange.custom"))
                    .selected(self.custom_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.custom_open = true;
                        cx.notify();
                    })),
            );
        let panel = div()
            .v_flex()
            .gap_3()
            .w(px(if self.custom_open { 480. } else { 264. }))
            .child(tabs);
        if !self.custom_open {
            return panel
                .child(
                    div()
                        .v_flex()
                        .gap_1()
                        .child(
                            Button::new("time-any")
                                .small()
                                .ghost()
                                .w_full()
                                .text_color(cx.theme().foreground)
                                .accessibility_label(tr(language, "agentSearch.unlimited"))
                                .child(div().w_full().child(tr(language, "agentSearch.unlimited")))
                                .selected(!self.has_value(cx))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.clear(window, cx)),
                                ),
                        )
                        .children(PRESETS.iter().enumerate().map(|(index, (key, _))| {
                            Button::new(("time-preset", index))
                                .small()
                                .ghost()
                                .w_full()
                                .debug_selector(move || format!("time-preset-{index}").into())
                                .text_color(cx.theme().foreground)
                                .accessibility_label(tr(language, key))
                                .child(div().w_full().child(tr(language, key)))
                                .selected(self.preset == Some(index))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.apply_preset(index, window, cx)
                                }))
                        })),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(tr(language, "timeRange.basis")),
                )
                .into_any_element();
        }
        panel
            .debug_selector(|| "time-range-custom-panel".into())
            .child(
                div()
                    .h_flex()
                    .items_start()
                    .gap_4()
                    .debug_selector(|| "time-range-calendar".into())
                    .children((0..2).map(|end| {
                        div()
                            .v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .debug_selector(move || format!("time-range-endpoint-{end}").into())
                            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(tr(
                                language,
                                if end == 0 {
                                    "timeRange.from"
                                } else {
                                    "timeRange.until"
                                },
                            )))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        self.endpoint(end, cx)
                                            .map(|value| value.format("%Y-%m-%d").to_string())
                                            .unwrap_or_else(|| {
                                                tr(language, "timeRange.date").to_owned()
                                            }),
                                    ),
                            )
                            .child(
                                div().flex().justify_center().child(
                                    Calendar::new(&self.calendars[end])
                                        .small()
                                        .border_0()
                                        .p_0()
                                        .w(px(196.)),
                                ),
                            )
                            .child(div().h_flex().gap_1().children((0..3).map(|part| {
                                div()
                                    .debug_selector(move || {
                                        format!("time-range-time-{end}-{part}").into()
                                    })
                                    .h_flex()
                                    .gap_1()
                                    .children((part > 0).then_some(div().child(":")))
                                    .child(
                                        Select::new(&self.times[end][part])
                                            .small()
                                            .w(px(64.))
                                            .menu_max_h(px(200.))
                                            .disabled(self.endpoint(end, cx).is_none())
                                            .accessibility_label(format!(
                                                "{} {}",
                                                tr(
                                                    language,
                                                    if end == 0 {
                                                        "timeRange.from"
                                                    } else {
                                                        "timeRange.until"
                                                    }
                                                ),
                                                tr(
                                                    language,
                                                    [
                                                        "timeRange.hour",
                                                        "timeRange.minute",
                                                        "timeRange.second"
                                                    ][part]
                                                )
                                            )),
                                    )
                            })))
                    })),
            )
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .pt_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(tr(language, "timeRange.basis")),
                    )
                    .child(
                        Button::new("clear-time-range")
                            .small()
                            .ghost()
                            .debug_selector(|| "time-range-clear".into())
                            .label(tr(language, "timeRange.clear"))
                            .disabled(!self.has_value(cx))
                            .on_click(cx.listener(|this, _, window, cx| this.clear(window, cx))),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, NaiveDate};
    use core::prelude::v1::test;

    #[gpui_kit::test]
    fn picker_interactions_preserve_seconds_and_emit_changes(cx: &mut TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(560.), px(500.)), |window, cx| {
            DateTimeRangePicker::new(Language::En, window, cx)
        });
        let picker = window.root(cx).unwrap();
        let changes = Rc::new(Cell::new(0));
        cx.update(|cx| {
            let changes = changes.clone();
            cx.subscribe(&picker, move |_, _: &DateTimeRangeEvent, _| {
                changes.set(changes.get() + 1);
            })
            .detach();
        });
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        let custom = visual.debug_bounds("time-custom").unwrap();
        visual.simulate_click(custom.center(), Default::default());
        visual.run_until_parked();
        let calendar = visual.debug_bounds("time-range-calendar").unwrap();
        assert!(calendar.right() <= px(560.));
        assert!(visual.debug_bounds("time-range-time-1-2").is_some());

        let time = NaiveDate::from_ymd_opt(2024, 2, 29)
            .unwrap()
            .and_hms_opt(12, 30, 45)
            .unwrap();
        window
            .update(cx, |picker, window, cx| {
                picker.set_range(Some(time), Some(time), window, cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(changes.get(), 1);
        picker.read_with(cx, |picker, cx| {
            let (from, until) = picker.bounds(cx).unwrap();
            assert_eq!(until.unwrap() - from.unwrap(), 1000);
        });

        let back = visual.debug_bounds("time-range-back").unwrap();
        visual.simulate_click(back.center(), Default::default());
        visual.run_until_parked();
        let preset = visual.debug_bounds("time-preset-0").unwrap();
        visual.simulate_click(preset.center(), Default::default());
        visual.run_until_parked();
        picker.read_with(cx, |picker, cx| {
            assert_eq!(picker.preset, Some(0));
            let (from, until) = picker.bounds(cx).unwrap();
            assert_eq!(until.unwrap() - from.unwrap(), 60_000);
        });
        assert_eq!(changes.get(), 2);

        // A confirmed seconds selection changes the wall time and clears the preset.
        window
            .update(cx, |picker, window, cx| {
                let seconds = picker.times[0][2].clone();
                seconds.update(cx, |state, cx| {
                    let old: usize = state.selected_value().unwrap().parse().unwrap();
                    let value = (old + 1) % 60;
                    state.set_selected_index(Some(IndexPath::new(value)), window, cx);
                    cx.emit(SelectEvent::Confirm(Some(format!("{value:02}"))));
                });
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(changes.get(), 3);
        assert!(picker.read_with(cx, |picker, _| picker.preset.is_none()));
        window
            .update(cx, |picker, window, cx| picker.clear(window, cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(changes.get(), 4);
        picker.read_with(cx, |picker, cx| {
            assert!(!picker.has_value(cx));
            assert_eq!(picker.bounds(cx).unwrap(), (None, None));
            assert_eq!(picker.times[0][2].read(cx).selected_value().unwrap(), "00");
            assert_eq!(picker.times[1][2].read(cx).selected_value().unwrap(), "59");
        });
    }

    #[gpui_kit::test]
    fn custom_calendars_edit_independent_endpoints_and_show_time(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(560.), px(500.)), |window, cx| {
            DateTimeRangePicker::new(Language::En, window, cx)
        });
        let picker = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        let custom = visual.debug_bounds("time-custom").unwrap();
        let first_preset = visual.debug_bounds("time-preset-0").unwrap();
        assert!(
            custom.bottom() < first_preset.top(),
            "custom must be visible above the preset list"
        );
        visual.simulate_click(custom.center(), Default::default());
        visual.run_until_parked();
        let start = NaiveDate::from_ymd_opt(2024, 2, 28).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 3, 2).unwrap();
        for (index, date) in [(0, start), (1, end)] {
            picker.update(cx, |picker, cx| {
                picker.calendars[index].update(cx, |calendar, cx| {
                    calendar.activate_date(date, cx);
                });
            });
            visual.run_until_parked();
        }
        picker.read_with(cx, |picker, cx| {
            assert_eq!(picker.endpoint(0, cx), start.and_hms_opt(0, 0, 0));
            assert_eq!(picker.endpoint(1, cx), end.and_hms_opt(23, 59, 59));
        });
        for language in [Language::Zh, Language::En] {
            picker.update(cx, |picker, cx| picker.set_language(language, cx));
            visual.run_until_parked();
            let from = visual.debug_bounds("time-range-endpoint-0").unwrap();
            let until = visual.debug_bounds("time-range-endpoint-1").unwrap();
            let panel = visual.debug_bounds("time-range-custom-panel").unwrap();
            assert_eq!(from.top(), until.top());
            assert!(from.right() < until.left());
            assert!(until.right() <= panel.right());
            for selector in ["time-range-time-0-2", "time-range-time-1-2"] {
                let time = visual.debug_bounds(selector).unwrap();
                assert!(time.right() <= panel.right() && time.bottom() <= panel.bottom());
            }
            let clear = visual.debug_bounds("time-range-clear").unwrap();
            assert!(panel.right() <= px(560.) && clear.bottom() - panel.top() < px(420.));
        }
        // Changing the start independently must preserve the end and report invalid order.
        picker.update(cx, |picker, cx| {
            picker.calendars[0].update(cx, |calendar, cx| {
                calendar.activate_date(end.succ_opt().unwrap(), cx);
            });
        });
        visual.run_until_parked();
        picker.read_with(cx, |picker, cx| {
            assert_eq!(picker.endpoint(1, cx), end.and_hms_opt(23, 59, 59));
            assert_eq!(picker.bounds(cx), Err("timeRange.invalidOrder"));
        });
        let clear = visual.debug_bounds("time-range-clear").unwrap();
        visual.simulate_click(clear.center(), Default::default());
        visual.run_until_parked();
        picker.read_with(cx, |picker, cx| {
            assert_eq!(picker.bounds(cx), Ok((None, None)));
            assert_eq!(picker.times[0][0].read(cx).selected_value().unwrap(), "00");
            assert_eq!(picker.times[1][0].read(cx).selected_value().unwrap(), "23");
            assert!(picker.custom_open);
        });
    }

    #[test]
    fn rolling_presets_have_exact_durations_at_second_precision() {
        let now = FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(2024, 3, 1, 0, 0, 0)
            .unwrap()
            .with_nanosecond(123_000_000)
            .unwrap();
        for (index, (_, seconds)) in PRESETS
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 5 && *i != 6)
        {
            let (from, until) = preset_range(index, now).unwrap();
            assert_eq!((until - from).num_seconds(), *seconds);
            assert_eq!(until.nanosecond(), 0);
            assert_eq!(until.second(), 1);
        }
    }

    #[test]
    fn calendar_presets_handle_leap_day() {
        let now = FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(2024, 3, 1, 12, 0, 0)
            .unwrap();
        let (from, until) = preset_range(6, now).unwrap();
        assert_eq!(
            from.date_naive(),
            NaiveDate::from_ymd_opt(2024, 2, 29).unwrap()
        );
        assert_eq!(until.date_naive(), now.date_naive());
        let (from, until) = preset_range(5, now).unwrap();
        assert_eq!(from.hour(), 0);
        assert_eq!(
            until.date_naive(),
            NaiveDate::from_ymd_opt(2024, 3, 2).unwrap()
        );
    }

    #[test]
    fn bounds_include_last_second_and_reject_reverse_order() {
        let time = NaiveDate::from_ymd_opt(2024, 2, 29)
            .unwrap()
            .and_hms_opt(12, 30, 45)
            .unwrap();
        let (from, until) = resolve_bounds(Some(time), Some(time)).unwrap();
        assert_eq!(until.unwrap() - from.unwrap(), 1000);
        assert_eq!(
            resolve_bounds(Some(time + Duration::seconds(1)), Some(time)),
            Err("timeRange.invalidOrder")
        );
        assert_eq!(resolve_bounds(None, None), Ok((None, None)));
        assert!(resolve_bounds(Some(time), None).unwrap().1.is_none());
    }
}
