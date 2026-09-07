#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentSegment {
    Markdown(String),
    Mermaid(String),
}

pub fn is_valid_mermaid_syntax(content: &str) -> bool {
    let first_line = content
        .trim()
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    [
        "graph ",
        "flowchart ",
        "sequencediagram",
        "classdiagram",
        "statediagram",
        "erdiagram",
        "gantt",
        "pie",
        "requirementdiagram",
        "gitgraph",
        "c4context",
        "c4container",
        "c4component",
        "c4dynamic",
        "c4deployment",
        "mindmap",
        "timeline",
        "quadrantchart",
        "xychart-beta",
        "sankey-beta",
        "block-beta",
        "packet-beta",
        "architecture-beta",
    ]
    .iter()
    .any(|prefix| first_line.starts_with(prefix))
}

pub fn split_mermaid_blocks(content: &str) -> Vec<ContentSegment> {
    let mut segments = Vec::new();
    let mut markdown = Vec::new();
    let mut diagram = Vec::new();
    let mut in_mermaid = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if !in_mermaid && trimmed.eq_ignore_ascii_case("```mermaid") {
            if !markdown.is_empty() {
                segments.push(ContentSegment::Markdown(markdown.join("\n")));
                markdown.clear();
            }
            in_mermaid = true;
        } else if in_mermaid && trimmed == "```" {
            let source = diagram.join("\n");
            if is_valid_mermaid_syntax(&source) {
                segments.push(ContentSegment::Mermaid(source));
            } else {
                segments.push(ContentSegment::Markdown(format!("```text\n{source}\n```")));
            }
            diagram.clear();
            in_mermaid = false;
        } else if in_mermaid {
            diagram.push(line);
        } else {
            markdown.push(line);
        }
    }

    if in_mermaid {
        markdown.push("```mermaid");
        markdown.append(&mut diagram);
    }
    if !markdown.is_empty() {
        segments.push(ContentSegment::Markdown(markdown.join("\n")));
    }
    if segments.is_empty() && !content.is_empty() {
        segments.push(ContentSegment::Markdown(content.to_owned()));
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_markdown_and_mermaid() {
        let parts = split_mermaid_blocks("before\n```mermaid\ngraph TD\nA-->B\n```\nafter");
        assert_eq!(parts.len(), 3);
        assert!(matches!(&parts[1], ContentSegment::Mermaid(value) if value.contains("A-->B")));
    }

    #[test]
    fn preserves_an_unclosed_fence_as_markdown() {
        let parts = split_mermaid_blocks("```mermaid\ngraph TD");
        assert!(
            matches!(&parts[0], ContentSegment::Markdown(value) if value.starts_with("```mermaid"))
        );
    }

    #[test]
    fn treats_invalid_mermaid_as_a_plain_code_block() {
        let parts = split_mermaid_blocks("```mermaid\nnot a diagram\n```");
        assert!(matches!(
            &parts[0],
            ContentSegment::Markdown(value) if value == "```text\nnot a diagram\n```"
        ));
    }
}
