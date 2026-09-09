use crate::i18n::tr;
use chrono::{DateTime, Duration, Local, NaiveDateTime, NaiveTime, TimeZone, Timelike};
use gpui_kit::base::{Disableable, Selectable, StyledExt};
use gpui_kit::component::{
    ActiveTheme, IndexPath, Sizable,
    button::{Button, ButtonVariants},
    calendar::{Calendar, CalendarState, Date},
    select::{Select, SelectEvent, SelectState},
    switch::Switch,
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
    calendar: Entity<CalendarState>,
    time_enabled: bool,
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
        let calendar = cx.new(|cx| {
            let mut state = CalendarState::new(window, cx);
            state.set_date(Date::Range(None, None), window, cx);
            state
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
        subscriptions.push(cx.observe(&calendar, |this, _, cx| {
            this.selection_changed(cx);
        }));
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
            calendar,
            time_enabled: false,
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
        let range = self.calendar.read(cx).date();
        let date = if end == 0 { range.start() } else { range.end() }?;
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
        self.calendar.update(cx, |state, cx| {
            state.set_date(
                Date::Range(from.map(|v| v.date()), until.map(|v| v.date())),
                window,
                cx,
            );
        });
        self.time_enabled = from.is_some_and(|v| v.time() != NaiveTime::MIN)
            || until.is_some_and(|v| v.time() != NaiveTime::from_hms_opt(23, 59, 59).unwrap());
        for (end, value) in [from, until].into_iter().enumerate() {
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

    fn set_time_enabled(&mut self, enabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.time_enabled == enabled {
            return;
        }
        if !enabled {
            let from = self
                .endpoint(0, cx)
                .map(|v| v.date().and_time(NaiveTime::MIN));
            let until = self
                .endpoint(1, cx)
                .map(|v| v.date().and_hms_opt(23, 59, 59).unwrap());
            self.set_range(from, until, window, cx);
        }
        self.time_enabled = enabled;
        cx.notify();
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
        if !self.custom_open {
            return div()
                .w(px(232.))
                .v_flex()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .pb_2()
                        .child(tr(language, "timeRange.basis")),
                )
                .child(
                    Button::new("time-any")
                        .small()
                        .ghost()
                        .w_full()
                        .text_color(cx.theme().foreground)
                        .accessibility_label(tr(language, "agentSearch.unlimited"))
                        .child(div().w_full().child(tr(language, "agentSearch.unlimited")))
                        .selected(!self.has_value(cx))
                        .on_click(cx.listener(|this, _, window, cx| this.clear(window, cx))),
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
                }))
                .child(
                    Button::new("time-custom")
                        .small()
                        .ghost()
                        .w_full()
                        .debug_selector(|| "time-custom".into())
                        .text_color(cx.theme().foreground)
                        .accessibility_label(tr(language, "timeRange.custom"))
                        .child(div().w_full().child(tr(language, "timeRange.custom")))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.custom_open = true;
                            cx.notify();
                        })),
                )
                .into_any_element();
        }
        div()
            .v_flex()
            .gap_2()
            .w(px(288.))
            .debug_selector(|| "time-range-custom-panel".into())
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(
                        Button::new("time-back")
                            .small()
                            .ghost()
                            .debug_selector(|| "time-range-back".into())
                            .label(tr(language, "timeRange.presets"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.custom_open = false;
                                cx.notify();
                            })),
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
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(language, "timeRange.chooseRange")),
            )
            .child(
                div()
                    .flex()
                    .justify_center()
                    .debug_selector(|| "time-range-calendar".into())
                    .child(
                        Calendar::new(&self.calendar)
                            .small()
                            .border_0()
                            .p_0()
                            .w(px(196.)),
                    ),
            )
            .child(div().text_sm().child(self.summary(cx)))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(language, "timeRange.basis")),
            )
            .child(
                div().debug_selector(|| "time-range-set-time".into()).child(
                    Switch::new("set-time")
                        .small()
                        .checked(self.time_enabled)
                        .label(tr(language, "timeRange.setTime"))
                        .on_click(cx.listener(|this, enabled, window, cx| {
                            this.set_time_enabled(*enabled, window, cx)
                        })),
                ),
            )
            .children(self.time_enabled.then(|| {
                div().v_flex().gap_1().children((0..2).map(|end| {
                    div()
                        .h_flex()
                        .gap_1()
                        .child(
                            div()
                                .w(px(34.))
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(tr(
                                    language,
                                    if end == 0 {
                                        "timeRange.from"
                                    } else {
                                        "timeRange.until"
                                    },
                                )),
                        )
                        .children((0..3).map(|part| {
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
                                        .accessibility_label(tr(
                                            language,
                                            [
                                                "timeRange.hour",
                                                "timeRange.minute",
                                                "timeRange.second",
                                            ][part],
                                        )),
                                )
                        }))
                }))
            }))
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
        assert!(visual.debug_bounds("time-range-time-1-2").is_none());

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
    fn custom_calendar_defaults_to_days_and_time_is_optional(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(320.), px(500.)), |window, cx| {
            let mut picker = DateTimeRangePicker::new(Language::En, window, cx);
            picker.custom_open = true;
            picker
        });
        let picker = window.root(cx).unwrap();
        let start = NaiveDate::from_ymd_opt(2024, 2, 28).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 2, 29).unwrap();
        window
            .update(cx, |picker, _, cx| {
                picker.calendar.update(cx, |calendar, cx| {
                    calendar.activate_date(start, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        picker.read_with(cx, |picker, cx| {
            assert_eq!(picker.endpoint(0, cx), start.and_hms_opt(0, 0, 0))
        });
        window
            .update(cx, |picker, _, cx| {
                picker.calendar.update(cx, |calendar, cx| {
                    calendar.activate_date(end, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        picker.read_with(cx, |picker, cx| {
            assert_eq!(picker.endpoint(1, cx), end.and_hms_opt(23, 59, 59));
            assert!(!picker.time_enabled);
        });
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("time-range-time-0-0").is_none());
        let toggle = visual.debug_bounds("time-range-set-time").unwrap();
        visual.simulate_click(
            point(toggle.left() + px(8.), toggle.center().y),
            Default::default(),
        );
        visual.run_until_parked();
        assert!(picker.read_with(cx, |picker, _| picker.time_enabled));
        assert!(visual.debug_bounds("time-range-time-0-0").is_some());
        let panel = visual.debug_bounds("time-range-custom-panel").unwrap();
        let last = visual.debug_bounds("time-range-time-1-2").unwrap();
        assert!(
            last.bottom() - panel.top() < px(420.),
            "expanded custom content height {:?}",
            last.bottom() - panel.top()
        );
        assert!(panel.right() <= px(320.));
        window
            .update(cx, |picker, window, cx| {
                picker.set_range(
                    start.and_hms_opt(12, 30, 40),
                    end.and_hms_opt(15, 20, 30),
                    window,
                    cx,
                );
                assert!(picker.time_enabled);
                picker.set_time_enabled(false, window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        picker.read_with(cx, |picker, cx| {
            assert_eq!(picker.endpoint(0, cx), start.and_hms_opt(0, 0, 0));
            assert_eq!(picker.endpoint(1, cx), end.and_hms_opt(23, 59, 59));
            assert!(!picker.time_enabled);
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
