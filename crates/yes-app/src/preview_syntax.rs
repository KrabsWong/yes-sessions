use std::{
    ops::Range,
    path::Path,
    sync::LazyLock,
    time::{Duration, Instant},
};

use syntect::{
    easy::ScopeRangeIterator,
    parsing::{ParseState, Scope, ScopeStack, SyntaxSet},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenKind {
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
}

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static TOKEN_SCOPES: LazyLock<Vec<(Scope, TokenKind)>> = LazyLock::new(|| {
    [
        ("comment", TokenKind::Comment),
        ("string", TokenKind::String),
        ("constant.numeric", TokenKind::Number),
        ("constant.language", TokenKind::Keyword),
        ("keyword", TokenKind::Keyword),
        ("storage", TokenKind::Keyword),
        ("entity.name.type", TokenKind::Type),
        ("entity.name.class", TokenKind::Type),
        ("support.type", TokenKind::Type),
        ("support.class", TokenKind::Type),
        ("entity.name.function", TokenKind::Function),
        ("support.function", TokenKind::Function),
    ]
    .into_iter()
    .map(|(name, kind)| (Scope::new(name).expect("valid built-in scope"), kind))
    .collect()
});

fn token_kind(stack: &ScopeStack) -> Option<TokenKind> {
    TOKEN_SCOPES.iter().find_map(|(prefix, kind)| {
        stack
            .as_slice()
            .iter()
            .any(|scope| prefix.is_prefix_of(*scope))
            .then_some(*kind)
    })
}

/// Parse once on the preview worker; ranges are UTF-8 byte offsets in each input line.
/// Preserve parser state between lines so comments and strings can span multiple rows.
pub(crate) fn highlight_lines(
    path: &Path,
    lines: &[String],
) -> Vec<Vec<(Range<usize>, TokenKind)>> {
    let mut result = vec![Vec::new(); lines.len()];
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return result;
    };
    let Some(syntax) = SYNTAXES.find_syntax_by_extension(extension) else {
        return result;
    };
    let mut parser = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let started = Instant::now();
    let mut bytes = 0usize;

    for (index, line) in lines.iter().enumerate() {
        bytes = bytes.saturating_add(line.len());
        // Stop instead of skipping a row: its contents may change multiline parser state.
        if index >= 10_000
            || bytes > 2 * 1024 * 1024
            || line.len() > 8 * 1024
            || started.elapsed() > Duration::from_millis(200)
        {
            break;
        }
        let mut source = line.clone();
        if !source.ends_with('\n') {
            source.push('\n');
        }
        let Ok(operations) = parser.parse_line(&source, &SYNTAXES) else {
            break;
        };
        let mut spans: Vec<(Range<usize>, TokenKind)> = Vec::new();
        for (range, operation) in ScopeRangeIterator::new(&operations, &source) {
            if stack.apply(operation).is_err() {
                return result;
            }
            let range = range.start.min(line.len())..range.end.min(line.len());
            if range.is_empty() {
                continue;
            }
            let Some(kind) = token_kind(&stack) else {
                continue;
            };
            if let Some((previous, previous_kind)) = spans.last_mut()
                && *previous_kind == kind
                && previous.end == range.start
            {
                previous.end = range.end;
            } else {
                spans.push((range, kind));
            }
        }
        result[index] = spans;
    }
    result
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{TokenKind, highlight_lines};

    #[test]
    fn unicode_ranges_are_bytes_and_preserve_multiline_comments() {
        let lines = [
            "/* 中文注释".to_string(),
            "仍在注释中 */ let name = \"你好🦀\";".to_string(),
            "fn answer() -> u32 { 42 }".to_string(),
        ];
        let spans = highlight_lines(Path::new("example.rs"), &lines);
        for (line, spans) in lines.iter().zip(&spans) {
            for (range, _) in spans {
                assert!(line.get(range.clone()).is_some());
            }
        }
        assert!(spans[0].iter().any(|(_, kind)| *kind == TokenKind::Comment));
        assert!(spans[1].iter().any(|(range, kind)| {
            *kind == TokenKind::Comment && lines[1][range.clone()].contains("仍在注释中")
        }));
        assert!(spans[1].iter().any(|(range, kind)| {
            *kind == TokenKind::String && lines[1][range.clone()].contains("你好🦀")
        }));
        assert!(spans[2].iter().any(|(range, kind)| {
            *kind == TokenKind::Number && &lines[2][range.clone()] == "42"
        }));
        assert!(!spans[2].iter().any(|(_, kind)| *kind == TokenKind::Comment));
    }

    #[test]
    fn unknown_extensions_and_oversized_rows_stay_plain() {
        let lines = vec!["fn main() {}".to_string(); 2];
        assert!(
            highlight_lines(Path::new("file.unknown-extension"), &lines)
                .iter()
                .all(Vec::is_empty)
        );
        let lines = vec!["x".repeat(8 * 1024 + 1), "fn main() {}".to_string()];
        assert!(
            highlight_lines(Path::new("file.rs"), &lines)
                .iter()
                .all(Vec::is_empty)
        );
    }
}
