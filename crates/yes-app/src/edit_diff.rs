//! Bounded, in-memory comparison of a historical Edit tool's replacement strings.
const MAX_BYTES: usize = 128 * 1024;
const MAX_LINES: usize = 1_000;
const MAX_CELLS: usize = 250_000;
const MAX_ROWS: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Context,
    Removed,
    Added,
    Omitted,
}

#[derive(Debug)]
pub(crate) struct Row {
    pub kind: Kind,
    pub text: String,
    pub no_newline: bool,
}

pub(crate) struct EditDiff {
    pub rows: Vec<Row>,
    pub added: usize,
    pub removed: usize,
    pub limited: bool,
    pub complete_input: bool,
}

fn bounded_lines(text: &str) -> (Vec<&str>, bool) {
    let mut end = text.len().min(MAX_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut lines = text[..end]
        .split_inclusive('\n')
        .take(MAX_LINES + 1)
        .collect::<Vec<_>>();
    let complete = end == text.len() && lines.len() <= MAX_LINES;
    lines.truncate(MAX_LINES);
    (lines, complete)
}

pub(crate) fn compare(old: &str, new: &str) -> EditDiff {
    let (old, old_complete) = bounded_lines(old);
    let (new, new_complete) = bounded_lines(new);
    let mut rows = Vec::new();
    let mut long_line = false;
    let mut push = |kind, text: &str| {
        let no_newline = !text.ends_with('\n') && old_complete && new_complete;
        let text = text.trim_end_matches('\n');
        let mut end = text.len().min(2_048);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        long_line |= end < text.len();
        let truncated = end < text.len();
        let mut text = text[..end].to_owned();
        if truncated {
            text.push('…');
        }
        rows.push(Row {
            kind,
            text,
            no_newline,
        });
    };
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    for line in &old[..prefix] {
        push(Kind::Context, line);
    }
    let old = &old[prefix..];
    let new = &new[prefix..];
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (old_mid, old_tail) = old.split_at(old.len() - suffix);
    let new_mid = &new[..new.len() - suffix];
    let width = new_mid.len() + 1;
    let coarse = (old_mid.len() + 1) * width > MAX_CELLS;
    if coarse {
        // A valid whole-block replacement avoids quadratic work for unrelated large inputs.
        for line in old_mid {
            push(Kind::Removed, line);
        }
        for line in new_mid {
            push(Kind::Added, line);
        }
    } else {
        let mut lengths = vec![0u16; (old_mid.len() + 1) * width];
        for i in (0..old_mid.len()).rev() {
            for j in (0..new_mid.len()).rev() {
                lengths[i * width + j] = if old_mid[i] == new_mid[j] {
                    lengths[(i + 1) * width + j + 1] + 1
                } else {
                    lengths[(i + 1) * width + j].max(lengths[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < old_mid.len() || j < new_mid.len() {
            if i < old_mid.len() && j < new_mid.len() && old_mid[i] == new_mid[j] {
                push(Kind::Context, old_mid[i]);
                i += 1;
                j += 1;
            } else if i < old_mid.len()
                && (j == new_mid.len()
                    || lengths[(i + 1) * width + j] >= lengths[i * width + j + 1])
            {
                push(Kind::Removed, old_mid[i]);
                i += 1;
            } else {
                push(Kind::Added, new_mid[j]);
                j += 1;
            }
        }
    }
    for line in old_tail {
        push(Kind::Context, line);
    }
    let added = rows.iter().filter(|row| row.kind == Kind::Added).count();
    let removed = rows.iter().filter(|row| row.kind == Kind::Removed).count();
    // Keep a small amount of context around each edit so a long unchanged prefix
    // cannot push the actual replacement out of the bounded preview.
    if added + removed > 0 {
        let mut keep = vec![false; rows.len()];
        for (index, row) in rows.iter().enumerate() {
            if row.kind != Kind::Context {
                let end = (index + 4).min(keep.len());
                keep[index.saturating_sub(3)..end].fill(true);
            }
        }
        let mut compact = Vec::new();
        for (row, keep) in rows.into_iter().zip(keep) {
            if keep {
                compact.push(row);
            } else if !compact
                .last()
                .is_some_and(|row: &Row| row.kind == Kind::Omitted)
            {
                compact.push(Row {
                    kind: Kind::Omitted,
                    text: "…".into(),
                    no_newline: false,
                });
            }
        }
        rows = compact;
    }
    let complete_input = old_complete && new_complete;
    let limited = !complete_input || coarse || long_line || rows.len() > MAX_ROWS;
    if rows.len() > MAX_ROWS {
        rows.drain(MAX_ROWS / 2..rows.len() - MAX_ROWS / 2);
        rows.insert(
            MAX_ROWS / 2,
            Row {
                kind: Kind::Omitted,
                text: "…".into(),
                no_newline: false,
            },
        );
    }
    EditDiff {
        rows,
        added,
        removed,
        limited,
        complete_input,
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, MAX_ROWS, compare};

    #[test]
    fn preserves_common_lines_and_counts_insertions_and_deletions() {
        let diff = compare("a\nold\nz\n", "a\nnew\nextra\nz\n");
        assert_eq!((diff.removed, diff.added), (1, 2));
        assert_eq!(
            diff.rows.iter().map(|row| row.kind).collect::<Vec<_>>(),
            [
                Kind::Context,
                Kind::Removed,
                Kind::Added,
                Kind::Added,
                Kind::Context
            ]
        );
        assert_eq!(compare("", "hello").added, 1);
        assert_eq!(compare("hello", "").removed, 1);
        assert_eq!(compare("hello", "hello").added, 0);
        let newline = compare("hello", "hello\n");
        assert!(newline.rows[0].no_newline);
        assert!(!newline.rows[1].no_newline);
        // A missing final newline is a real modification.
        assert_eq!(
            (
                compare("hello", "hello\n").removed,
                compare("hello", "hello\n").added
            ),
            (1, 1)
        );
    }

    #[test]
    fn long_unchanged_context_does_not_hide_the_actual_edit() {
        let prefix = "same\n".repeat(450);
        let suffix = "tail\n".repeat(450);
        let diff = compare(
            &format!("{prefix}old\n{suffix}"),
            &format!("{prefix}new\n{suffix}"),
        );
        assert!(
            diff.rows
                .iter()
                .any(|row| row.kind == Kind::Removed && row.text == "old")
        );
        assert!(
            diff.rows
                .iter()
                .any(|row| row.kind == Kind::Added && row.text == "new")
        );
        assert!(diff.rows.len() < 20);
    }

    #[test]
    fn large_unicode_inputs_and_unrelated_lines_stay_bounded() {
        let text = "你好世界".repeat(100_000);
        let diff = compare(&text, "new");
        assert!(!diff.complete_input);
        assert!(diff.limited);
        let old = (0..900).map(|i| format!("old{i}\n")).collect::<String>();
        let new = (0..900).map(|i| format!("new{i}\n")).collect::<String>();
        let diff = compare(&old, &new);
        assert!(diff.complete_input && diff.limited);
        assert_eq!((diff.removed, diff.added), (900, 900));
        assert!(diff.rows.len() <= MAX_ROWS + 1);
        assert!(diff.rows.iter().any(|row| row.kind == Kind::Omitted));
    }
}
