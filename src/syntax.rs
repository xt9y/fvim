use crate::diagnostics::Diagnostic;
use std::path::Path;
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

fn grammar(ft: &str) -> Option<(Language, String)> {
    Some(match ft {
        "c" => (
            tree_sitter_c::LANGUAGE.into(),
            tree_sitter_c::HIGHLIGHT_QUERY.into(),
        ),
        "cpp" => (
            tree_sitter_cpp::LANGUAGE.into(),
            format!(
                "{}\n{}",
                tree_sitter_c::HIGHLIGHT_QUERY,
                tree_sitter_cpp::HIGHLIGHT_QUERY
            ),
        ),
        "lua" => (
            tree_sitter_lua::LANGUAGE.into(),
            tree_sitter_lua::HIGHLIGHTS_QUERY.into(),
        ),
        "zig" => (
            tree_sitter_zig::LANGUAGE.into(),
            tree_sitter_zig::HIGHLIGHTS_QUERY.into(),
        ),
        "odin" => (
            tree_sitter_odin::LANGUAGE.into(),
            tree_sitter_odin::HIGHLIGHTS_QUERY.into(),
        ),
        "bash" | "sh" => (
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY.into(),
        ),
        "json" => (
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY.into(),
        ),
        "markdown" => (
            tree_sitter_md::LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into(),
        ),
        "hlsl" => (
            tree_sitter_hlsl::LANGUAGE_HLSL.into(),
            tree_sitter_c::HIGHLIGHT_QUERY.into(),
        ),
        _ => return None,
    })
}
fn group(capture: &str) -> Option<&'static str> {
    if capture.starts_with("markup.heading") || capture.starts_with("text.title") {
        return Some("Function");
    }
    if capture.starts_with("markup.raw") || capture.starts_with("text.literal") {
        return Some("String");
    }
    let name = capture.split('.').next().unwrap_or(capture);
    match name {
        "comment" => Some("Comment"),
        "string" | "character" => Some("String"),
        "number" | "float" | "boolean" => Some("Number"),
        "keyword" | "conditional" | "repeat" | "exception" => Some("Keyword"),
        "type" => Some("Type"),
        "function" => Some("Function"),
        _ => None,
    }
}
fn spans_for(
    node: tree_sitter::Node<'_>,
    lines: &[&str],
    group: &'static str,
    spans: &mut Vec<Span>,
) {
    let start = node.start_position();
    let end = node.end_position();
    for (row, line) in lines
        .iter()
        .enumerate()
        .take(end.row.min(lines.len().saturating_sub(1)) + 1)
        .skip(start.row)
    {
        let a = if row == start.row {
            start.column.min(line.len())
        } else {
            0
        };
        let b = if row == end.row {
            end.column.min(line.len())
        } else {
            line.len()
        };
        if a <= b && line.is_char_boundary(a) && line.is_char_boundary(b) {
            spans.push(Span {
                row,
                start: line[..a].chars().count(),
                end: line[..b].chars().count(),
                group,
            });
        }
    }
}
pub fn analyze(path: &Path, ft: &str, text: &str, enabled: bool) -> (Vec<Span>, Vec<Diagnostic>) {
    let mut spans = vec![];
    let mut errors = vec![];
    if !enabled {
        return (spans, errors);
    }
    let lines: Vec<_> = text.split('\n').collect();
    if ft == "llvm" {
        use std::sync::OnceLock;
        static TOKENS: OnceLock<regex::Regex> = OnceLock::new();
        let tokens = TOKENS.get_or_init(|| regex::Regex::new(r#";[^\n]*|c?"(?:[^"\\]|\\.)*"|\b(?:i[0-9]+|void|half|float|double|fp128|ptr|label|metadata|token)\b|\b(?:define|declare|global|constant|target|datalayout|triple|attributes|ret|br|switch|call|invoke|load|store|alloca|getelementptr|phi|select|icmp|fcmp|add|sub|mul|udiv|sdiv|and|or|xor|shl|lshr|ashr|bitcast|trunc|zext|sext|unreachable|to|inbounds|align|nounwind)\b|[-+]?[0-9]+(?:\.[0-9]+)?|[@%][-a-zA-Z$._0-9]+"#).unwrap());
        for (row, line) in lines.iter().enumerate() {
            for token in tokens.find_iter(line) {
                let text = token.as_str();
                let group = if text.starts_with(';') {
                    "Comment"
                } else if text.contains('"') {
                    "String"
                } else if text.starts_with('@') {
                    "Function"
                } else if text.starts_with('%') {
                    "Type"
                } else if text
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_digit() || ch == '-' || ch == '+')
                {
                    "Number"
                } else if text.starts_with('i') && text[1..].chars().all(|ch| ch.is_ascii_digit())
                    || [
                        "void", "half", "float", "double", "fp128", "ptr", "label", "metadata",
                        "token",
                    ]
                    .contains(&text)
                {
                    "Type"
                } else {
                    "Keyword"
                };
                spans.push(Span {
                    row,
                    start: line[..token.start()].chars().count(),
                    end: line[..token.end()].chars().count(),
                    group,
                });
            }
        }
        return (spans, errors);
    }
    let Some((language, query)) = grammar(ft) else {
        return (spans, errors);
    };
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return (spans, errors);
    }
    let Some(tree) = parser.parse(text, None) else {
        return (spans, errors);
    };
    if let Ok(query) = Query::new(&language, &query) {
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, tree.root_node(), text.as_bytes());
        while let Some(m) = matches.next() {
            for capture in m.captures() {
                if let Some(group) = group(query.capture_names()[capture.index as usize]) {
                    spans_for(capture.node, &lines, group, &mut spans);
                }
            }
        }
    }
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if node.is_error() || node.is_missing() {
            let p = node.start_position();
            let e = node.end_position();
            let col = lines
                .get(p.row)
                .map_or(0, |line| line[..p.column.min(line.len())].chars().count());
            let end_col = lines.get(e.row).map_or(col + 1, |line| {
                line[..e.column.min(line.len())].chars().count()
            });
            if errors.len() < 2000 {
                errors.push(Diagnostic {
                    path: Some(path.to_owned()),
                    row: p.row,
                    col,
                    end_row: e.row,
                    end_col: if e.row == p.row {
                        end_col.max(col + 1)
                    } else {
                        end_col
                    },
                    severity: 1,
                    source: "syntax".into(),
                    message: if node.is_missing() {
                        format!("Missing {}", node.kind())
                    } else {
                        "Syntax error".into()
                    },
                });
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                spans.sort_by_key(|s| (s.row, s.start));
                return (spans, errors);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Span {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub group: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_configured_grammars_load_and_c_errors_have_real_positions() {
        for ft in [
            "c", "cpp", "lua", "zig", "odin", "bash", "json", "markdown", "hlsl",
        ] {
            let (language, query) = grammar(ft).unwrap();
            Parser::new().set_language(&language).unwrap();
            Query::new(&language, &query).unwrap_or_else(|error| panic!("{ft}: {error}"));
        }
        let (spans, errors) = analyze(
            std::path::Path::new("file.c"),
            "c",
            "int main() { return ; broken( }",
            true,
        );
        assert!(!errors.is_empty());
        assert!(spans.iter().any(|s| s.group == "Keyword"));
        let (_, errors) = analyze(std::path::Path::new("file.json"), "json", "{\"x\": }", true);
        assert!(!errors.is_empty());
    }
}
