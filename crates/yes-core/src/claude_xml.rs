//! Claude's file/directory message envelope, not a general XML interpreter.
//! Payloads and paths remain inert transcript text; no entities or files are resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeXmlSegment<'a> {
    Text(&'a str),
    File {
        path: &'a str,
        content: &'a str,
    },
    Directory {
        path: &'a str,
        entries: Vec<&'a str>,
    },
}

fn tag<'a>(input: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    let input = input.trim_start();
    let body = input.strip_prefix(&format!("<{name}>"))?;
    let end = body.find(&format!("</{name}>"))?;
    Some((&body[..end], &body[end + name.len() + 3..]))
}

/// Recognize complete, consecutive path/type/payload envelopes. Anything malformed
/// or unknown is retained verbatim, including text surrounding valid envelopes.
pub fn parse_claude_xml(input: &str) -> Vec<ClaudeXmlSegment<'_>> {
    let mut segments = Vec::new();
    let mut cursor = 0;
    let mut text_start = 0;
    while let Some(relative) = input[cursor..].find("<path>") {
        let start = cursor + relative;
        cursor = start + "<path>".len();
        let Some((path, rest)) = tag(&input[start..], "path") else {
            continue;
        };
        if path.trim().is_empty() || path.contains('<') {
            continue;
        }
        let Some((kind, rest)) = tag(rest, "type") else {
            continue;
        };
        let (segment, rest) = match kind.trim() {
            "file" => match tag(rest, "content") {
                Some((content, rest)) => (
                    ClaudeXmlSegment::File {
                        path: path.trim(),
                        content,
                    },
                    rest,
                ),
                None => continue,
            },
            "directory" => match tag(rest, "entries") {
                Some((entries, rest)) => (
                    ClaudeXmlSegment::Directory {
                        path: path.trim(),
                        entries: entries
                            .lines()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .collect(),
                    },
                    rest,
                ),
                None => continue,
            },
            _ => continue,
        };
        if text_start < start {
            segments.push(ClaudeXmlSegment::Text(&input[text_start..start]));
        }
        segments.push(segment);
        cursor = input.len() - rest.len();
        text_start = cursor;
    }
    if text_start < input.len() {
        segments.push(ClaudeXmlSegment::Text(&input[text_start..]));
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::{ClaudeXmlSegment, parse_claude_xml};

    #[test]
    fn mixed_cards_keep_surrounding_text_and_code_verbatim() {
        let input = "Before\n<path>/src/a.rs</path>\n<type>file</type>\n<content>\n  <path>literal</path> &lt;\n</content>\nBetween\n<path>src/</path><type>directory</type><entries> a.rs\n sub/\n</entries>\nAfter";
        assert_eq!(
            parse_claude_xml(input),
            vec![
                ClaudeXmlSegment::Text("Before\n"),
                ClaudeXmlSegment::File {
                    path: "/src/a.rs",
                    content: "\n  <path>literal</path> &lt;\n"
                },
                ClaudeXmlSegment::Text("\nBetween\n"),
                ClaudeXmlSegment::Directory {
                    path: "src/",
                    entries: vec!["a.rs", "sub/"]
                },
                ClaudeXmlSegment::Text("\nAfter"),
            ]
        );
    }

    #[test]
    fn malformed_unknown_and_empty_paths_remain_text() {
        for input in [
            "<path>x</path><type>file</type><content>missing",
            "<path>x</path><type>unknown</type><content>x</content>",
            "<path></path><type>file</type><content>x</content>",
            "<!DOCTYPE x SYSTEM 'file:///secret'>plain",
        ] {
            assert_eq!(parse_claude_xml(input), vec![ClaudeXmlSegment::Text(input)]);
        }
        assert_eq!(
            parse_claude_xml("<path>x</path><type>file</type><content></content>"),
            vec![ClaudeXmlSegment::File {
                path: "x",
                content: ""
            }]
        );
    }
}
