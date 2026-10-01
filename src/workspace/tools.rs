use super::*;
use crate::{
    editor::{Completion, CompletionMenu},
    tooling::{self, Event},
};
use serde_json::Value;

impl Workspace {
    pub(super) fn diagnostic_entries(&self) -> Vec<crate::picker::Entry> {
        let mut items: Vec<_> = self.diagnostics.values().flatten().collect();
        if self.settings.tooling.diagnostics.severity_sort {
            items.sort_by_key(|d| (d.severity, d.path.clone(), d.row, d.col));
        } else {
            items.sort_by_key(|d| (d.path.clone(), d.row, d.col));
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
                            .is_some_and(|stored| tooling::uri(p) == tooling::uri(stored))
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
        if idle {
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
                    if self.saved_generations.get(&id).copied().unwrap_or(0)
                        != e.buffer.save_generation
                    {
                        self.tools.saved(path.clone());
                        self.saved_generations.insert(id, e.buffer.save_generation);
                    }
                    continue;
                }
                e.semantic.clear();
                self.tool_version += 1;
                let absolute =
                    tooling::from_uri(&tooling::uri(path)).unwrap_or_else(|| path.clone());
                self.versions.insert(absolute.clone(), self.tool_version);
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
                if self.saved_generations.get(&id).copied().unwrap_or(0) != e.buffer.save_generation
                {
                    self.tools.saved(path.clone());
                    self.saved_generations.insert(id, e.buffer.save_generation);
                }
            }
        }
        for event in self.tools.poll() {
            match event {
                Event::Semantic {
                    path,
                    version,
                    spans,
                } => {
                    if self.versions.get(&path) != Some(&version) {
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
                    if self.versions.get(&path) != Some(&version) {
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
                        self.versions.get(&path).is_some_and(|current| v < *current)
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
                    if self.versions.get(&path) != Some(&version) {
                        continue;
                    }
                    let e = self.editor();
                    if e.buffer
                        .path
                        .as_ref()
                        .is_none_or(|p| tooling::uri(p) != tooling::uri(&path))
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
            let b = &mut self.editor_mut().buffer;
            apply_completion(b, item);
            return true;
        }
        self.editor_mut().completion = None;
        false
    }
}
fn apply_completion(b: &mut Buffer, item: &Completion) {
    let main_start = b.offset_at(item.start.0, item.start.1);
    let main_end = b.offset_at(item.end.0, item.end.1);
    let mut cursor = main_start + item.text.chars().count();
    let mut edits = vec![(main_start, main_end, item.text.as_str())];
    for edit in &item.additional {
        let start = b.offset_at(edit.start.0, edit.start.1);
        let end = b.offset_at(edit.end.0, edit.end.1);
        if end <= main_start {
            cursor = cursor + edit.text.chars().count() - (end - start);
        }
        edits.push((start, end, &edit.text));
    }
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    b.begin_change();
    for (start, end, text) in edits {
        b.replace(start, end, text);
    }
    b.set_offset(cursor);
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
            if item["insertTextFormat"] == 2 {
                return None;
            }
            let label = item["label"].as_str()?.to_owned();
            let text = item["textEdit"]["newText"]
                .as_str()
                .or_else(|| item["insertText"].as_str())
                .unwrap_or(&label)
                .to_owned();
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
        apply_completion(&mut e.buffer, &items[0]);
        assert_eq!(e.buffer.text(), "#include <vector>\n// heading\nvector");
        assert_eq!((e.buffer.row, e.buffer.col), (2, 6));
        e.buffer.end_change();
        e.buffer.undo();
        assert_eq!(e.buffer.text(), "// heading\nvec");
    }
}
