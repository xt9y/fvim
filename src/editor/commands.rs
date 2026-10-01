use super::{Confirmation, Editor, Mode};
use crate::buffer::{write_atomic, Buffer};
use crate::command::{self, Search, Substitute};
use crate::motion::{self, Motion};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;

impl Editor {
    pub(super) fn save(&mut self) {
        self.message = match self.buffer.save() {
            Ok(()) => "Saved.".to_owned(),
            Err(e) => format!("Save failed: {e}"),
        };
    }

    pub(super) fn start_search(&mut self, pattern: &str, forward: bool) {
        let pattern = if pattern.is_empty() {
            self.search
                .as_ref()
                .map(|s| s.pattern.as_str())
                .unwrap_or("")
        } else {
            pattern
        };
        match Search::new(pattern.to_owned(), forward) {
            Ok(search) => {
                self.search = Some(search);
                self.repeat_search(false);
            }
            Err(e) => {
                self.message = e;
                self.reset_command();
            }
        }
    }

    pub(super) fn repeat_search(&mut self, reverse: bool) {
        self.search_from(reverse, self.buffer.offset());
    }

    pub(super) fn keyword_search(&mut self, forward: bool) {
        let Some((start, end)) = motion::object(&self.buffer, 'w', false, 1) else {
            self.reset_command();
            return;
        };
        let word: String = self.buffer.chars_forward(start).take(end - start).collect();
        if !word.chars().all(|c| c.is_alphanumeric() || c == '_') {
            self.message = "No keyword under cursor".into();
            self.reset_command();
            return;
        }
        match Search::new(format!(r"\<{}\>", regex::escape(&word)), forward) {
            Ok(search) => {
                self.search = Some(search);
                self.search_from(false, start);
            }
            Err(e) => {
                self.message = e;
                self.reset_command();
            }
        }
    }

    fn search_from(&mut self, reverse: bool, position: usize) {
        let Some(search) = &self.search else {
            self.message = "No previous search".into();
            self.reset_command();
            return;
        };
        let destination = search.destination(
            &self.buffer.body(),
            position,
            search.forward ^ reverse,
            self.total_count(),
        );
        if let Some(offset) = destination {
            self.move_or_operate(Motion {
                offset,
                inclusive: false,
                linewise: false,
            });
        } else {
            self.message = "Pattern not found".into();
            self.reset_command();
        }
    }

    fn substitute(&mut self, text: &str, start: usize, end: usize) -> Result<(), String> {
        let sub = Substitute::parse(text, self.search.as_ref().map(|s| s.pattern.as_str()))?;
        let edits = sub.edits(&self.buffer.lines, start, end)?;
        self.search = Some(Search {
            pattern: sub.pattern,
            forward: true,
            regex: sub.regex,
        });
        if edits.is_empty() && !sub.suppress_error {
            return Err("Pattern not found".into());
        }
        if sub.count_only {
            self.message = format!("{} matches", edits.len());
            return Ok(());
        }
        self.buffer.begin_change();
        if sub.confirm && !edits.is_empty() {
            self.confirmation = Some(Confirmation {
                edits: edits.into(),
                delta: 0,
                applied: 0,
            });
            self.show_confirmation();
        } else {
            let n = edits.len();
            for edit in edits.into_iter().rev() {
                self.buffer.replace(edit.start, edit.end, &edit.replacement);
            }
            self.message = format!("{n} substitutions");
        }
        Ok(())
    }

    fn show_confirmation(&mut self) {
        if let Some(c) = &self.confirmation {
            if let Some(edit) = c.edits.front() {
                self.buffer
                    .set_offset(edit.start.saturating_add_signed(c.delta));
                self.message = format!("replace with {}? (y/n/a/q/l)", edit.replacement);
                self.clamp();
                return;
            }
        }
        if let Some(c) = self.confirmation.take() {
            self.message = format!("{} substitutions", c.applied);
        }
    }

    pub(super) fn confirm_key(&mut self, code: KeyCode) {
        let action = match code {
            KeyCode::Char(ch) => ch,
            KeyCode::Esc => 'q',
            _ => return,
        };
        if !matches!(action, 'y' | 'n' | 'a' | 'q' | 'l') {
            return;
        }
        let c = self.confirmation.as_mut().unwrap();
        if action == 'q' {
            c.edits.clear();
        } else {
            while let Some(edit) = c.edits.pop_front() {
                if action != 'n' {
                    self.buffer.replace(
                        edit.start.saturating_add_signed(c.delta),
                        edit.end.saturating_add_signed(c.delta),
                        &edit.replacement,
                    );
                    c.delta += edit.replacement.chars().count() as isize
                        - (edit.end - edit.start) as isize;
                    c.applied += 1;
                }
                if action != 'a' {
                    break;
                }
            }
            if action == 'l' {
                c.edits.clear();
            }
        }
        self.show_confirmation();
    }

    fn ex(&mut self, text: &str) -> bool {
        match self.execute_ex(text) {
            Ok(quit) => quit,
            Err(e) => {
                self.message = e;
                false
            }
        }
    }

    pub(super) fn execute_ex(&mut self, text: &str) -> Result<bool, String> {
        let (start, end, rest, has_range) = command::range(
            text,
            self.buffer.row,
            self.buffer.lines.len(),
            self.visual_range,
        )?;
        if rest.is_empty() {
            if has_range {
                self.buffer.row = end - 1;
                self.buffer.col = motion::first_nonblank(&self.buffer.lines[end - 1]);
            }
            return Ok(false);
        }
        if rest.starts_with('s')
            && rest
                .chars()
                .nth(1)
                .is_none_or(|ch| !ch.is_ascii_alphabetic())
        {
            self.substitute(rest, start, end)?;
            return Ok(false);
        }
        let split = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let name = &rest[..split];
        let argument = rest[split..].trim();
        let force = name.ends_with('!');
        let name = name.trim_end_matches('!');
        match name {
            "set" | "lua" | "source" | "luafile" | "colorscheme" => {
                if has_range {
                    return Err("Invalid range for configuration command".into());
                }
                self.config_command = Some(rest.into());
            }
            "q" | "quit" => {
                if self.buffer.dirty() && !force {
                    return Err("Unsaved changes. Use :q! to discard them.".into());
                }
                return Ok(true);
            }
            "w" | "write" | "wq" | "x" | "exit" => {
                if has_range {
                    return Err("Writing a range is not supported yet".into());
                }
                if !argument.is_empty() {
                    let path = PathBuf::from(argument);
                    let path = if path.exists() {
                        std::fs::canonicalize(&path).map_err(|e| e.to_string())?
                    } else {
                        path
                    };
                    let current = self
                        .buffer
                        .path
                        .as_ref()
                        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()));
                    if path.exists() && current.as_ref() != Some(&path) && !force {
                        return Err("File exists. Use :w! to overwrite it.".into());
                    }
                    if current.is_none() || current.as_ref() == Some(&path) {
                        if current.is_none() {
                            self.buffer.path = Some(path);
                        }
                        self.buffer
                            .save()
                            .map_err(|e| format!("Save failed: {e}"))?;
                    } else {
                        write_atomic(&path, self.buffer.text().as_bytes())
                            .map_err(|e| format!("Save failed: {e}"))?;
                    }
                } else if !matches!(name, "x" | "exit") || self.buffer.dirty() {
                    self.buffer
                        .save()
                        .map_err(|e| format!("Save failed: {e}"))?;
                }
                self.message = "Saved.".into();
                return Ok(matches!(name, "wq" | "x" | "exit"));
            }
            "e" | "edit" | "enew" => {
                if has_range {
                    return Err("Invalid range for edit".into());
                }
                if self.buffer.dirty() && !force {
                    return Err("Unsaved changes. Use :e! to discard them.".into());
                }
                let path = if name == "enew" {
                    None
                } else if argument.is_empty() {
                    self.buffer.path.clone()
                } else {
                    Some(PathBuf::from(argument))
                };
                self.buffer = Buffer::open(path).map_err(|e| format!("Open failed: {e}"))?;
                self.mode = Mode::Normal;
                self.visual_range = None;
            }
            "d" | "delete" | "y" | "yank" | ">" | "<" => {
                if !argument.is_empty() {
                    return Err("Unexpected command argument".into());
                }
                self.line_operator(name.chars().next().unwrap(), start, end);
            }
            "u" | "undo" => self.buffer.undo(),
            "red" | "redo" => self.buffer.redo(),
            _ => return Err(format!("Unknown command: {name}")),
        }
        Ok(false)
    }

    pub(super) fn prompt_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
                self.reset_command();
                self.finish();
            }
            KeyCode::Backspace => {
                self.prompt.as_mut().unwrap().text.pop();
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.prompt.as_mut().unwrap().text.push(ch)
            }
            KeyCode::Enter => {
                let p = self.prompt.take().unwrap();
                let quit = if p.kind == ':' {
                    self.ex(&p.text)
                } else {
                    self.start_search(&p.text, p.kind == '/');
                    false
                };
                self.reset_command();
                self.finish();
                return quit;
            }
            _ => {}
        }
        false
    }
}
