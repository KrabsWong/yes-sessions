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
    Property,
    Tag,
    Attribute,
    Heading,
    Bold,
    Italic,
    InlineCode,
    LinkText,
    LinkUri,
}

impl TokenKind {
    pub(crate) fn theme_key(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::String => "string",
            Self::Comment => "comment",
            Self::Number => "number",
            Self::Type => "type",
            Self::Function => "function",
            Self::Property => "property",
            Self::Tag => "tag",
            Self::Attribute => "attribute",
            Self::Heading => "title",
            Self::Bold => "emphasis.strong",
            Self::Italic => "emphasis",
            Self::InlineCode => "text.code.span",
            Self::LinkText => "link_text",
            Self::LinkUri => "link_uri",
        }
    }
}

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);
static TOKEN_SCOPES: LazyLock<Vec<(Scope, TokenKind)>> = LazyLock::new(|| {
    [
        ("markup.raw.inline", TokenKind::InlineCode),
        ("markup.underline.link", TokenKind::LinkUri),
        ("meta.link.inline.description", TokenKind::LinkText),
        ("markup.heading", TokenKind::Heading),
        ("markup.bold", TokenKind::Bold),
        ("markup.italic", TokenKind::Italic),
        ("comment", TokenKind::Comment),
        ("meta.mapping.key", TokenKind::Property),
        ("support.type.property-name", TokenKind::Property),
        ("entity.other.attribute-name", TokenKind::Attribute),
        ("entity.name.tag", TokenKind::Tag),
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
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let syntax = SYNTAXES
        .find_syntax_by_extension(filename)
        .or_else(|| SYNTAXES.find_syntax_by_extension(extension))
        .or_else(|| match extension {
            "jsx" => SYNTAXES.find_syntax_by_extension("tsx"),
            "mjs" | "cjs" => SYNTAXES.find_syntax_by_extension("js"),
            "mts" | "cts" => SYNTAXES.find_syntax_by_extension("ts"),
            _ if filename.starts_with("Dockerfile.") => {
                SYNTAXES.find_syntax_by_extension("Dockerfile")
            }
            _ => None,
        })
        .or_else(|| {
            lines
                .first()
                .and_then(|line| SYNTAXES.find_syntax_by_first_line(line))
        });
    let Some(syntax) = syntax else {
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
            // Lazy regex initialization can exceed the budget on the first lines,
            // especially in Vue. Always allow a small, size-bounded prefix to finish.
            || (index >= 32 && started.elapsed() > Duration::from_millis(200))
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
    fn common_languages_and_extensionless_files_are_highlighted() {
        for (name, source) in [
            ("main.rs", "fn main() { let count = 42; }"),
            ("main.ts", "const count: number = 42;"),
            ("server.mjs", "import http from 'node:http';"),
            ("server.cjs", "const http = require('node:http');"),
            ("config.yml", "name: \"hello\""),
            ("run.sh", "echo \"hello\""),
            ("run.bash", "echo \"hello\""),
            (".bashrc", "export PATH=\"$HOME/bin:$PATH\""),
            ("main.tsx", "const view = <div title=\"hello\">{42}</div>;"),
            ("main.jsx", "const view = <div title=\"hello\">{42}</div>;"),
            ("main.kt", "fun main() { val count = 42 }"),
            ("main.swift", "let count: Int = 42"),
            ("main.py", "def main(): return 42"),
            ("main.go", "package main; var count = 42"),
            ("main.java", "class Main { int count = 42; }"),
            ("main.m", "NSString *name = @\"hello\";"),
            ("main.cpp", "int main() { return 42; }"),
            ("Cargo.toml", "name = \"hello\""),
            ("file.json", "{\"name\": \"hello\", \"count\": 42}"),
            ("Dockerfile", "FROM rust:1.90"),
            ("Makefile", "all: build"),
            ("run", "#!/usr/bin/env python3\nprint(42)"),
        ] {
            let lines = source.lines().map(str::to_owned).collect::<Vec<_>>();
            let highlights = highlight_lines(Path::new(name), &lines);
            assert!(
                highlights.iter().any(|line| !line.is_empty()),
                "missing highlighting: {name}"
            );
            for (line, spans) in lines.iter().zip(highlights) {
                assert!(
                    spans
                        .iter()
                        .all(|(range, _)| line.get(range.clone()).is_some()),
                    "invalid UTF-8 range: {name}"
                );
            }
        }
    }

    #[test]
    fn token_colors_are_available_in_both_themes() {
        use gpui_kit::component::highlighter::HighlightTheme;
        let light = HighlightTheme::default_light();
        let dark = HighlightTheme::default_dark();
        for theme in [&light, &dark] {
            for kind in [
                TokenKind::Keyword,
                TokenKind::String,
                TokenKind::Comment,
                TokenKind::Number,
                TokenKind::Type,
                TokenKind::Function,
                TokenKind::Property,
                TokenKind::Tag,
                TokenKind::Attribute,
                TokenKind::Heading,
                TokenKind::Bold,
                TokenKind::Italic,
                TokenKind::InlineCode,
                TokenKind::LinkText,
                TokenKind::LinkUri,
            ] {
                assert!(
                    theme.style(kind.theme_key()).is_some(),
                    "missing token color: {kind:?}"
                );
            }
            assert_ne!(
                theme.style("keyword").unwrap().color,
                theme.style("string").unwrap().color
            );
        }
        assert_ne!(
            light.style("keyword").unwrap().color,
            dark.style("keyword").unwrap().color
        );
    }

    #[test]
    fn markdown_styles_and_vue_embedded_languages_are_preserved() {
        let lines = [
            "# Heading",
            "**bold** and *italic* and `code` and [link](https://example.com)",
        ]
        .map(str::to_owned);
        for extension in ["md", "markdown"] {
            let spans = highlight_lines(Path::new(&format!("README.{extension}")), &lines);
            for (text, kind, index) in [
                ("Heading", TokenKind::Heading, 0),
                ("bold", TokenKind::Bold, 1),
                ("italic", TokenKind::Italic, 1),
                ("code", TokenKind::InlineCode, 1),
                ("link", TokenKind::LinkText, 1),
                ("https://example.com", TokenKind::LinkUri, 1),
            ] {
                assert!(
                    spans[index].iter().any(|(range, token)| *token == kind
                        && lines[index][range.clone()].contains(text)),
                    "{extension}: missing {kind:?}"
                );
            }
        }
        let lines = [
            "<template>",
            "<div>Hello</div>",
            "</template>",
            "<script setup lang=\"ts\">",
            "const count: number = 42;",
            "</script>",
            "<style scoped>",
            ".app { color: red; }",
            "</style>",
        ]
        .map(str::to_owned);
        let spans = highlight_lines(Path::new("App.vue"), &lines);
        assert!(spans[1].iter().any(|(_, kind)| *kind == TokenKind::Tag));
        assert!(spans[4].iter().any(|(_, kind)| *kind == TokenKind::Keyword));
        assert!(spans[4].iter().any(|(_, kind)| *kind == TokenKind::Number));
        assert!(
            spans[7]
                .iter()
                .any(|(_, kind)| *kind == TokenKind::Property)
        );
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
