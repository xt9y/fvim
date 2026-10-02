use super::*;
use crate::{
    editor::{Completion, CompletionMenu, SnippetStop},
    tooling::{self, Event},
};
use serde_json::Value;

impl Workspace {
    pub(super) fn diagnostic_entries(&self) -> Vec<crate::picker::Entry> {
        let mut items: Vec<_> = self.diagnostics.values().flatten().collect();
        if self.settings.tooling.diagnostics.severity_sort {
            items.sort_by(|a, b| {
                a.severity
                    .cmp(&b.severity)
                    .then_with(|| a.path.cmp(&b.path))
                    .then(a.row.cmp(&b.row))
                    .then(a.col.cmp(&b.col))
            });
        } else {
            items.sort_by(|a, b| {
                a.path
                    .cmp(&b.path)
                    .then(a.row.cmp(&b.row))
                    .then(a.col.cmp(&b.col))
            });
        }
        items
            .into_iter()
            .map(|d| crate::picker::Entry {
                path: d.path.clone().unwrap_or_default(),
                row: d.row,
                col: d.col,
                label: d.label(),
            })
            .collect()
    }
    pub(super) fn refresh_diagnostics(&mut self) {
        for e in &mut self.buffers {
            if e.title.as_deref() == Some("[Build]")
                || (e.mode == Mode::Insert && !self.settings.tooling.diagnostics.update_in_insert)
            {
                continue;
            }
            e.diagnostics = self
                .diagnostics
                .values()
                .flatten()
                .filter(|d| {
                    d.path.as_ref().is_some_and(|p| {
                        e.buffer
                            .path
                            .as_ref()
                            .is_some_and(|stored| tooling::same_path(p, stored))
                    })
                })
                .cloned()
                .collect();
            e.diagnostics.sort_by_key(|d| (d.severity, d.row, d.col));
        }
        if self
            .picker
            .as_ref()
            .is_some_and(|p| p.title == "Diagnostics")
        {
            let entries = self.diagnostic_entries();
            self.picker.as_mut().unwrap().replace_entries(entries);
        }
    }
    pub(super) fn cursor_diagnostics(&mut self) -> bool {
        let e = self.editor();
        let lines: Vec<_> = e
            .diagnostics
            .iter()
            .filter(|d| d.contains(e.buffer.row, e.buffer.col))
            .map(|d| format!("{} [{}] {}", d.sign(), d.source, d.message))
            .collect();
        if lines.is_empty() {
            return false;
        }
        self.editor_mut().popup = Some(lines);
        true
    }
    pub fn poll(&mut self) -> bool {
        let mut redraw = false;
        let mut diagnostic_changed = false;
        if let Some(build) = &mut self.build {
            let (changed, finished) = build.poll();

            if changed {
                redraw = true;
                let e = &mut self.buffers[build.buffer];
                let (row, col) = (e.buffer.row, e.buffer.col);
                e.buffer = Buffer::from_text(&crate::diagnostics::plain_output(&build.text));
                e.terminal = Some(build.screen().clone());
                e.buffer.row = row.min(e.buffer.lines.len() - 1);
                e.buffer.col = col.min(e.buffer.lines[e.buffer.row].chars().count());
                if finished {
                    e.terminal = None;
                    e.mode = Mode::Normal;
                    e.message = "Build finished. :bprevious returns to source.".into();
                }
                e.diagnostics = build
                    .text
                    .lines()
                    .enumerate()
                    .flat_map(|(row, line)| {
                        crate::diagnostics::compiler(&self.root, line)
                            .into_iter()
                            .map(move |mut d| {
                                d.row = row;
                                d.end_row = row;
                                d.col = 0;
                                d.end_col = line.chars().count();
                                d
                            })
                    })
                    .collect();
                let errors = crate::diagnostics::compiler(&self.root, &build.text);
                self.diagnostics
                    .insert((self.root.clone(), "compiler".into()), errors);
                diagnostic_changed = true;
            }
            if finished {
                self.tools.project_built(self.root.clone());
                self.build = None;
                self.terminal_prefix = false;
            }
        }
        let idle = self.last_input.elapsed().as_millis() >= self.settings.tooling.delay_ms as u128;
        for (id, e) in self.buffers.iter_mut().enumerate() {
            let Some(path) = &e.buffer.path else {
                continue;
            };
            let dirty = e.buffer.dirty();
            if self
                .synced
                .get(&id)
                .is_some_and(|(revision, stored, _, _)| {
                    *revision == e.buffer.revision && stored == path
                })
            {
                if self.saved_generations.get(&id).copied().unwrap_or(0) != e.buffer.save_generation
                {
                    self.tools.saved(path.clone());
                    self.saved_generations.insert(id, e.buffer.save_generation);
                }
                continue;
            }
            e.semantic.clear();
            self.tool_version += 1;
            let absolute = tooling::normalized_path(path);
            self.versions
                .insert(tooling::path_key(&absolute), self.tool_version);
            self.synced.insert(
                id,
                (e.buffer.revision, path.clone(), self.tool_version, dirty),
            );
            self.tools.document(tooling::Document {
                path: absolute,
                version: self.tool_version,
                text: e.buffer.text().replace("\r\n", "\n"),
                filetype: self.settings.filetype(Some(path)),
                cursor: if e.mode == Mode::Insert {
                    Some((e.buffer.row, e.buffer.col))
                } else {
                    None
                },
            });
            if self.saved_generations.get(&id).copied().unwrap_or(0) != e.buffer.save_generation {
                self.tools.saved(path.clone());
                self.saved_generations.insert(id, e.buffer.save_generation);
            }
        }
        while let Some(event) = self.tools.try_event() {
            match event {
                Event::Semantic {
                    path,
                    version,
                    spans,
                } => {
                    if self.versions.get(&tooling::path_key(&path)) != Some(&version) {
                        continue;
                    }
                    for (id, e) in self.buffers.iter_mut().enumerate() {
                        if self.synced.get(&id).is_some_and(|(revision, _, v, _)| {
                            *v == version && *revision == e.buffer.revision
                        }) {
                            e.semantic = spans.clone();
                            redraw = true;
                        }
                    }
                }
                Event::Status(message) => {
                    if self.statuses.len() >= 20 {
                        self.statuses.remove(0);
                    }
                    self.statuses.push(message);
                    redraw = true;
                }
                Event::Syntax {
                    path,
                    version,
                    spans,
                    diagnostics,
                } => {
                    if self.versions.get(&tooling::path_key(&path)) != Some(&version) {
                        continue;
                    }
                    let mut applied = false;
                    for (id, e) in self.buffers.iter_mut().enumerate() {
                        if self.synced.get(&id).is_some_and(|(revision, _, v, _)| {
                            *v == version && *revision == e.buffer.revision
                        }) {
                            e.syntax = spans.clone();
                            applied = true;
                        }
                    }
                    if applied {
                        self.diagnostics
                            .insert((path, "syntax".into()), diagnostics);
                        diagnostic_changed = true;
                        redraw = true;
                    }
                }
                Event::Diagnostics {
                    path,
                    version,
                    source,
                    items,
                } => {
                    if version.is_some_and(|v| {
                        self.versions.get(&tooling::path_key(&path)).is_some_and(|current| v < *current)
                    }) {
                        continue;
                    }
                    self.diagnostics.insert((path, source), items);
                    diagnostic_changed = true;
                    redraw = true;
                }
                Event::Response {
                    method,
                    path,
                    version,
                    row,
                    col,
                    value,
                } => {
                    if self.versions.get(&tooling::path_key(&path)) != Some(&version) {
                        continue;
                    }
                    let e = self.editor();
                    if e.buffer
                        .path
                        .as_ref()
                        .is_none_or(|p| !tooling::same_path(p, &path))
                    {
                        continue;
                    }
                    if e.buffer.row != row || e.buffer.col != col {
                        continue;
                    }
                    if self
                        .synced
                        .get(&self.windows[self.active].as_ref().unwrap().buffer)
                        .is_none_or(|(revision, _, _, _)| *revision != e.buffer.revision)
                    {
                        continue;
                    }
                    match method.as_str() {
                        "textDocument/completion" if e.mode == Mode::Insert => {
                            let items = completion_items(e, &value, row, col);
                            if !items.is_empty() {
                                self.editor_mut().completion = Some(CompletionMenu {
                                    items,
                                    selected: None,
                                });
                                redraw = true;
                            }
                        }
                        "textDocument/hover" => {
                            let content = &value["contents"];
                            let text = if let Some(s) = content.as_str() {
                                s.to_owned()
                            } else if let Some(s) = content["value"].as_str() {
                                s.to_owned()
                            } else if let Some(a) = content.as_array() {
                                a.iter()
                                    .map(|s| {
                                        s.as_str().or_else(|| s["value"].as_str()).unwrap_or("")
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            } else {
                                String::new()
                            };
                            if !text.is_empty() {
                                self.editor_mut().popup =
                                    Some(text.lines().map(str::to_owned).collect());
                                redraw = true;
                            }
                        }
                        "textDocument/definition" | "textDocument/references" => {
                            let entries = locations(&value);
                            if method == "textDocument/definition" && entries.len() == 1 {
                                let entry = &entries[0];
                                if self.open(Some(entry.path.clone())).is_ok() {
                                    let e = self.editor_mut();
                                    e.buffer.row = entry.row.min(e.buffer.lines.len() - 1);
                                    e.buffer.col = entry.col.min(
                                        e.buffer.lines[e.buffer.row]
                                            .chars()
                                            .count()
                                            .saturating_sub(1),
                                    );
                                }
                            } else {
                                self.picker = Some(crate::picker::Picker::from_entries(
                                    "Locations",
                                    entries,
                                    &self.settings,
                                ));
                            }
                            redraw = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        if diagnostic_changed {
            self.refresh_diagnostics();
        }
        if idle
            && self.settings.tooling.diagnostics.float
            && self.editor().mode == Mode::Normal
            && self.editor().prompt.is_none()
            && self.editor().popup.is_none()
            && self.picker.is_none()
        {
            redraw |= self.cursor_diagnostics();
        }
        redraw
    }
    pub(super) fn completion_key(&mut self, key: KeyEvent) -> bool {
        if self.editor().snippet_active() && matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.editor_mut().completion = None;
            return false;
        }
        if self.editor().completion.is_none() {
            return false;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Tab
            || key.code == KeyCode::BackTab
            || (ctrl && matches!(key.code, KeyCode::Char('n' | 'p')))
        {
            let menu = self.editor_mut().completion.as_mut().unwrap();
            let reverse = key.code == KeyCode::BackTab || key.code == KeyCode::Char('p');
            menu.selected = Some(match menu.selected {
                None => {
                    if reverse {
                        menu.items.len() - 1
                    } else {
                        0
                    }
                }
                Some(i) => {
                    if reverse {
                        (i + menu.items.len() - 1) % menu.items.len()
                    } else {
                        (i + 1) % menu.items.len()
                    }
                }
            });
            return true;
        }
        let accept = (key.code == KeyCode::Enter
            && self
                .editor()
                .completion
                .as_ref()
                .unwrap()
                .selected
                .is_some())
            || (ctrl && key.code == KeyCode::Char('y'));
        if accept {
            let menu = self.editor_mut().completion.take().unwrap();
            let item = &menu.items[menu.selected.unwrap_or(0)];
            apply_completion(self.editor_mut(), item);
            return true;
        }
        self.editor_mut().completion = None;
        false
    }
}
fn shift_offset(value: usize, delta: isize) -> usize {
    if delta >= 0 {
        value.saturating_add(delta as usize)
    } else {
        value.saturating_sub(delta.unsigned_abs())
    }
}

fn completion_indent(e: &Editor, item: &Completion) -> String {
    let Some(line) = e.buffer.lines.get(item.start.0) else {
        return String::new();
    };
    let indent: String = line.chars().take_while(|ch| ch.is_whitespace()).collect();
    if item.start.1 >= indent.chars().count() {
        indent
    } else {
        String::new()
    }
}

fn indent_snippet(text: &str, stops: &mut [SnippetStop], indent: &str) -> String {
    if indent.is_empty() || !text.contains('\n') {
        return text.to_owned();
    }
    let width = indent.chars().count();
    let extra_before =
        |offset: usize| text.chars().take(offset).filter(|ch| *ch == '\n').count() * width;
    for stop in stops {
        let (start, end) = (stop.start, stop.end);
        stop.start += extra_before(start);
        stop.end += extra_before(end);
    }
    let mut out = String::with_capacity(text.len() + text.matches('\n').count() * indent.len());
    for ch in text.chars() {
        out.push(ch);
        if ch == '\n' {
            out.push_str(indent);
        }
    }
    out
}

fn apply_completion(e: &mut Editor, item: &Completion) {
    let main_start = e.buffer.offset_at(item.start.0, item.start.1);
    let main_end = e.buffer.offset_at(item.end.0, item.end.1);
    let (parsed, mut stops) = parse_snippet(&item.text);
    let text = indent_snippet(&parsed, &mut stops, &completion_indent(e, item));
    let mut shift = 0isize;
    let mut edits = vec![(main_start, main_end, text.clone())];
    for edit in &item.additional {
        let start = e.buffer.offset_at(edit.start.0, edit.start.1);
        let end = e.buffer.offset_at(edit.end.0, edit.end.1);
        if end <= main_start {
            shift += edit.text.chars().count() as isize - (end - start) as isize;
        }
        edits.push((start, end, edit.text.clone()));
    }
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    e.buffer.begin_change();
    for (start, end, replacement) in edits {
        e.buffer.replace(start, end, &replacement);
    }
    let applied_start = shift_offset(main_start, shift);
    let end = applied_start + text.chars().count();
    for stop in &mut stops {
        stop.start += applied_start;
        stop.end += applied_start;
    }
    if stops.is_empty() {
        e.buffer.set_offset(end);
    } else {
        e.activate_snippet(stops);
    }
}

fn matching_brace(chars: &[char], mut index: usize) -> Option<usize> {
    let mut depth = 1usize;
    while index < chars.len() {
        if chars[index] == '\\' {
            index += 2;
            continue;
        }
        if chars[index] == '$' && chars.get(index + 1) == Some(&'{') {
            depth += 1;
            index += 2;
            continue;
        }
        if chars[index] == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

fn parse_snippet_chars(chars: &[char]) -> (String, Vec<SnippetStop>) {
    let mut text = String::new();
    let mut stops = vec![];
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            text.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if chars[i] != '$' {
            text.push(chars[i]);
            i += 1;
            continue;
        }
        if chars.get(i + 1).is_some_and(|ch| ch.is_ascii_digit()) {
            let mut j = i + 1;
            while chars.get(j).is_some_and(|ch| ch.is_ascii_digit()) {
                j += 1;
            }
            let index = chars[i + 1..j]
                .iter()
                .collect::<String>()
                .parse()
                .unwrap_or(0);
            let at = text.chars().count();
            stops.push(SnippetStop {
                index,
                start: at,
                end: at,
            });
            i = j;
            continue;
        }
        if chars.get(i + 1) != Some(&'{') {
            text.push('$');
            i += 1;
            continue;
        }
        let Some(close) = matching_brace(chars, i + 2) else {
            text.push('$');
            i += 1;
            continue;
        };
        let mut j = i + 2;
        let digit_start = j;
        while chars.get(j).is_some_and(|ch| ch.is_ascii_digit()) {
            j += 1;
        }
        if j == digit_start {
            let inner = &chars[i + 2..close];
            if let Some(colon) = inner.iter().position(|ch| *ch == ':') {
                let start = text.chars().count();
                let (default, mut nested) = parse_snippet_chars(&inner[colon + 1..]);
                text.push_str(&default);
                for stop in &mut nested {
                    stop.start += start;
                    stop.end += start;
                }
                stops.extend(nested);
            } else {
                text.extend(inner.iter());
            }
            i = close + 1;
            continue;
        }
        let index = chars[digit_start..j]
            .iter()
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        let start = text.chars().count();
        match chars.get(j) {
            Some('}') => {
                stops.push(SnippetStop {
                    index,
                    start,
                    end: start,
                });
                i = j + 1;
            }
            Some(':') => {
                let (default, mut nested) = parse_snippet_chars(&chars[j + 1..close]);
                text.push_str(&default);
                let end = text.chars().count();
                stops.push(SnippetStop { index, start, end });
                for stop in &mut nested {
                    stop.start += start;
                    stop.end += start;
                }
                stops.extend(nested);
                i = close + 1;
            }
            Some('|') => {
                let mut choice = String::new();
                let mut k = j + 1;
                while k < close {
                    if chars[k] == '\\' && k + 1 < close {
                        choice.push(chars[k + 1]);
                        k += 2;
                        continue;
                    }
                    if chars[k] == ',' || (chars[k] == '|' && k + 1 == close) {
                        break;
                    }
                    choice.push(chars[k]);
                    k += 1;
                }
                text.push_str(&choice);
                stops.push(SnippetStop {
                    index,
                    start,
                    end: text.chars().count(),
                });
                i = close + 1;
            }
            _ => {
                text.push('$');
                i += 1;
            }
        }
    }
    (text, stops)
}

fn parse_snippet(source: &str) -> (String, Vec<SnippetStop>) {
    let chars: Vec<char> = source.chars().collect();
    let (text, mut stops) = parse_snippet_chars(&chars);
    if !stops.iter().any(|stop| stop.index == 0) {
        let end = text.chars().count();
        stops.push(SnippetStop {
            index: 0,
            start: end,
            end,
        });
    }
    stops.sort_by_key(|stop| (stop.index == 0, stop.index, stop.start));
    stops.dedup_by_key(|stop| stop.index);
    (text, stops)
}
fn position(e: &Editor, p: &Value) -> Option<(usize, usize)> {
    let row = p["line"].as_u64()? as usize;
    let line = e.buffer.lines.get(row)?;
    let col = tooling::utf16_col(line, p["character"].as_u64()? as usize);
    Some((row, col))
}
fn completion_items(e: &Editor, value: &Value, row: usize, col: usize) -> Vec<Completion> {
    let Some(items) = value.as_array().or_else(|| value["items"].as_array()) else {
        return vec![];
    };
    let line = &e.buffer.lines[row];
    let prefix: String = line.chars().take(col).collect();
    let length = prefix
        .chars()
        .rev()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
        .count();
    items
        .iter()
        .filter_map(|item| {
            let insert_label = item["label"].as_str()?.to_owned();
            let detail = item["labelDetails"]["detail"]
                .as_str()
                .or_else(|| item["detail"].as_str())
                .filter(|detail| !detail.trim().is_empty() && *detail != insert_label.as_str());
            let label = detail.map_or_else(
                || insert_label.clone(),
                |detail| format!("{insert_label}  {detail}"),
            );
            let mut text = item["textEdit"]["newText"]
                .as_str()
                .or_else(|| item["insertText"].as_str())
                .unwrap_or(&insert_label)
                .to_owned();
            // CompletionItem.insertTextFormat defaults to PlainText. Escape the
            // two characters interpreted by our internal snippet grammar so a
            // plain completion is inserted byte-for-byte instead of expanded.
            if item["insertTextFormat"].as_u64() != Some(2) {
                text = text.replace('\\', "\\\\").replace('$', "\\$");
            }
            let range = &item["textEdit"]["range"];
            let (start, end) = if range.is_object() {
                (position(e, &range["start"])?, position(e, &range["end"])?)
            } else {
                ((row, col - length), (row, col))
            };
            if start > end {
                return None;
            }
            let mut additional = vec![];
            if let Some(edits) = item["additionalTextEdits"].as_array() {
                for edit in edits {
                    additional.push(crate::editor::CompletionEdit {
                        start: position(e, &edit["range"]["start"])?,
                        end: position(e, &edit["range"]["end"])?,
                        text: edit["newText"].as_str()?.into(),
                    });
                }
            }
            let mut ranges = vec![(start, end)];
            ranges.extend(additional.iter().map(|edit| (edit.start, edit.end)));
            ranges.sort_unstable();
            if ranges.iter().any(|(a, b)| a > b)
                || ranges
                    .windows(2)
                    .any(|w| w[0].1 > w[1].0 || w[0].0 == w[1].0)
            {
                return None;
            }
            Some(Completion {
                additional,
                label,
                text,
                start,
                end,
            })
        })
        .take(200)
        .collect()
}
fn locations(value: &Value) -> Vec<crate::picker::Entry> {
    let values = if let Some(a) = value.as_array() {
        a.clone()
    } else if value.is_object() {
        vec![value.clone()]
    } else {
        vec![]
    };
    values
        .iter()
        .filter_map(|value| {
            let uri = value["uri"]
                .as_str()
                .or_else(|| value["targetUri"].as_str())?;
            let path = tooling::from_uri(uri)?;
            let range = if value["range"].is_object() {
                &value["range"]
            } else {
                &value["targetSelectionRange"]
            };
            let row = range["start"]["line"].as_u64()? as usize;
            let units = range["start"]["character"].as_u64()? as usize;
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let line = text.lines().nth(row).unwrap_or("");
            let col = tooling::utf16_col(line, units);
            Some(crate::picker::Entry {
                label: format!("{}:{}:{} {line}", path.display(), row + 1, col + 1),
                path,
                row,
                col,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn completion_applies_include_edits_and_tracks_cursor_and_undo() {
        let mut e = Editor::new(Buffer::from_text("// heading\nvec"));
        e.buffer.row = 1;
        e.buffer.col = 3;
        let items = completion_items(
            &e,
            &json!([{"label":"vector", "textEdit":{"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":3}},"newText":"vector"},"additionalTextEdits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"#include <vector>\n"}]}]),
            1,
            3,
        );
        apply_completion(&mut e, &items[0]);
        assert_eq!(e.buffer.text(), "#include <vector>\n// heading\nvector");
        assert_eq!((e.buffer.row, e.buffer.col), (2, 6));
        e.buffer.end_change();
        e.buffer.undo();
        assert_eq!(e.buffer.text(), "// heading\nvec");
    }

    #[test]
    fn snippet_parser_handles_defaults_tabstops_choices_and_final_cursor() {
        let (text, stops) = parse_snippet(r#"${1:name}($2, ${3|one\,two,three|})$0"#);
        assert_eq!(text, "name(, one,two)");
        assert_eq!(
            stops,
            vec![
                SnippetStop {
                    index: 1,
                    start: 0,
                    end: 4
                },
                SnippetStop {
                    index: 2,
                    start: 5,
                    end: 5
                },
                SnippetStop {
                    index: 3,
                    start: 7,
                    end: 14
                },
                SnippetStop {
                    index: 0,
                    start: 15,
                    end: 15
                },
            ]
        );
    }

    #[test]
    fn multiline_snippet_inherits_completion_line_indentation() {
        let mut e = Editor::new(Buffer::from_text("    fo"));
        e.mode = Mode::Insert;
        e.buffer.col = 6;
        let item = Completion {
            label: "for".into(),
            text: "for (${1:condition}) {\n    ${2:body}\n}$0".into(),
            start: (0, 4),
            end: (0, 6),
            additional: vec![],
        };

        apply_completion(&mut e, &item);
        assert_eq!(
            e.buffer.text(),
            "    for (condition) {\n        body\n    }"
        );
        assert_eq!((e.buffer.row, e.buffer.col), (0, 9));

        e.key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!((e.buffer.row, e.buffer.col), (1, 8));
    }

    #[test]
    fn plain_completion_preserves_dollar_and_backslash_text() {
        let mut e = Editor::new(Buffer::from_text("x"));
        e.buffer.col = 1;
        let items = completion_items(
            &e,
            &json!([{
                "label":"literal",
                "insertTextFormat":1,
                "insertText":"price$0\\path"
            }]),
            0,
            1,
        );
        apply_completion(&mut e, &items[0]);
        assert_eq!(e.buffer.text(), "price$0\\path");
        assert!(!e.snippet_active());
    }
}
