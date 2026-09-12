use gpui_kit::component::{ActiveTheme as _, tooltip::Tooltip};
use gpui_kit::*;
use yes_core::{Language, model::TokenUsage};

use crate::i18n::tr;

fn compact_count(value: u64) -> String {
    if value >= 999_950 {
        format!("{:.1} M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1} K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn grouped_count(value: u64) -> String {
    let digits = value.to_string();
    let mut result = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            result.push(',');
        }
        result.push(digit);
    }
    result
}

fn fields(usage: &TokenUsage, language: Language) -> Vec<(&'static str, String)> {
    let count = |value: Option<u64>| {
        value.map_or_else(
            || "—".into(),
            |value| {
                if value >= 1000 {
                    format!("{} ({})", compact_count(value), grouped_count(value))
                } else {
                    value.to_string()
                }
            },
        )
    };
    [
        ("usage.input", count(usage.input_tokens)),
        ("usage.output", count(usage.output_tokens)),
        ("usage.total", count(usage.total_tokens)),
        ("usage.cached", count(usage.cache_read_tokens)),
        (
            "usage.hitRate",
            usage
                .cache_hit_rate()
                .map_or_else(|| "—".into(), |rate| format!("{rate:.2}%")),
        ),
    ]
    .into_iter()
    .map(|(key, value)| (tr(language, key), value))
    .collect()
}

pub(crate) fn render(
    id: impl Into<ElementId>,
    usage: &TokenUsage,
    language: Language,
    cx: &App,
) -> impl IntoElement {
    let count = |value: Option<u64>| value.map_or_else(|| "—".into(), compact_count);
    let rate = usage
        .cache_hit_rate()
        .map_or_else(|| "—".into(), |rate| format!("{rate:.2}%"));
    let tooltip = fields(usage, language);
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .flex_none()
        .whitespace_nowrap()
        .text_size(px(10.))
        .font_weight(FontWeight::NORMAL)
        .text_color(cx.theme().muted_foreground)
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground.opacity(0.65))
                        .child("↑"),
                )
                .child(count(usage.input_tokens)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground.opacity(0.65))
                        .child("↓"),
                )
                .child(count(usage.output_tokens)),
        )
        .child(
            div()
                .text_color(cx.theme().muted_foreground.opacity(0.65))
                .child("·"),
        )
        .child(rate)
        .tooltip(move |window, cx| {
            let rows = tooltip.clone();
            Tooltip::element(move |_, cx| {
                div()
                    .flex()
                    .gap_4()
                    .py_1()
                    .text_size(px(12.))
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .text_color(cx.theme().muted_foreground)
                            .children(rows.iter().map(|(label, _)| div().child(*label))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .text_right()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().popover_foreground)
                            .children(rows.iter().map(|(_, value)| div().child(value.clone()))),
                    )
            })
            .build(window, cx)
        })
}

#[cfg(test)]
mod tests {
    use super::{compact_count, fields, grouped_count};
    use yes_core::{Language, model::TokenUsage};

    #[test]
    fn exact_counts_use_thousands_separators() {
        for (value, expected) in [
            (0, "0"),
            (117, "117"),
            (1000, "1,000"),
            (176372, "176,372"),
            (1234567890, "1,234,567,890"),
            (u64::MAX, "18,446,744,073,709,551,615"),
        ] {
            assert_eq!(grouped_count(value), expected);
        }
        let usage = TokenUsage {
            input_tokens: Some(176372),
            ..TokenUsage::default()
        };
        assert_eq!(
            fields(&usage, Language::En)[0],
            ("Input", "176.4 K (176,372)".into())
        );
    }

    #[test]
    fn counts_use_compact_units_with_one_decimal() {
        for (value, expected) in [
            (0, "0"),
            (62, "62"),
            (999, "999"),
            (1000, "1.0 K"),
            (34404, "34.4 K"),
            (999950, "1.0 M"),
            (1250000, "1.2 M"),
        ] {
            assert_eq!(compact_count(value), expected);
        }
    }

    #[test]
    fn cache_percentage_has_two_decimals_and_unknown_is_not_zero() {
        let mut usage = TokenUsage {
            input_tokens: Some(10000),
            output_tokens: Some(300),
            total_tokens: Some(10300),
            cache_read_tokens: Some(9212),
        };
        assert!(fields(&usage, Language::Zh).contains(&("缓存命中率", "92.12%".into())));
        usage.cache_read_tokens = Some(0);
        assert!(fields(&usage, Language::En).contains(&("Cache hit rate", "0.00%".into())));
        usage.cache_read_tokens = None;
        assert!(fields(&usage, Language::Zh).contains(&("缓存命中率", "—".into())));
        usage.input_tokens = Some(0);
        usage.cache_read_tokens = Some(0);
        assert!(fields(&usage, Language::En).contains(&("Cache hit rate", "—".into())));
    }
}
