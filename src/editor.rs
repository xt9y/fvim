mod commands;

use crate::buffer::Buffer;
use crate::command::{Edit, Search};
use crate::motion::{self, Motion};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Visual {
    Character,
    Line,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Normal,
    Insert,
    Visual(Visual),
}

#[derive(Clone)]
struct Register {
    text: String,
    kind: Visual,
}

#[derive(Clone)]
enum Input {
    Key(KeyEvent),
    Paste(String),
}

#[derive(Clone, Copy)]
enum Pending {
    G,
    Find(char),
    Object(bool),
    Register,
    Replace,
}

struct Confirmation {
    edits: VecDeque<Edit>,
    delta: isize,
    applied: usize,
}

pub struct Prompt {
    pub kind: char,
    pub text: String,
}

pub struct Editor {
    pub buffer: Buffer,
    pub mode: Mode,
    pub message: String,
    pub prompt: Option<Prompt>,
    pub page_rows: usize,
    search: Option<Search>,
    confirmation: Option<Confirmation>,
    visual_range: Option<(usize, usize)>,
    anchor: (usize, usize),
    goal: Option<usize>,
    count: usize,
    op: Option<(char, usize, bool)>,
    pending: Option<Pending>,
    find: Option<(char, char)>,
    registers: HashMap<char, Register>,
    register: char,
    register_pending: bool,
    recording: Vec<Input>,
    last_change: Vec<Input>,
    replaying: bool,
    discard_armed: bool,
    insert_count: usize,
    inserted: String,
    block_insert: Option<(usize, usize, usize)>,
}

impl Editor {
    pub fn new(buffer: Buffer) -> Self {
        Self {
            buffer,
            mode: Mode::Normal,
            message: String::new(),
            prompt: None,
            page_rows: 20,
            search: None,
            confirmation: None,
            visual_range: None,
            anchor: (0, 0),
            goal: None,
            count: 0,
            op: None,
            pending: None,
            find: None,
            registers: HashMap::new(),
            register: '"',
            register_pending: false,
            recording: Vec::new(),
            last_change: Vec::new(),
            replaying: false,
            discard_armed: false,
            insert_count: 1,
            inserted: String::new(),
            block_insert: None,
        }
    }

    pub fn mode_name(&self) -> &'static str {
        match self.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual(Visual::Character) => "VISUAL",
            Mode::Visual(Visual::Line) => "V-LINE",
            Mode::Visual(Visual::Block) => "V-BLOCK",
        }
    }

    pub fn command_line(&self) -> String {
        if let Some(p) = &self.prompt {
            format!("{}{}", p.kind, p.text)
        } else {
            self.message.clone()
        }
    }

    pub fn selected(&self, row: usize, col: usize) -> bool {
        match self.mode {
            Mode::Visual(Visual::Line) => (self.anchor.0.min(self.buffer.row)
                ..=self.anchor.0.max(self.buffer.row))
                .contains(&row),
            Mode::Visual(Visual::Block) => {
                (self.anchor.0.min(self.buffer.row)..=self.anchor.0.max(self.buffer.row))
                    .contains(&row)
                    && (self.anchor.1.min(self.buffer.col)..=self.anchor.1.max(self.buffer.col))
                        .contains(&col)
            }
            Mode::Visual(Visual::Character) => {
                let a = self.anchor;
                let b = (self.buffer.row, self.buffer.col);
                a.min(b) <= (row, col) && (row, col) <= a.max(b)
            }
            _ => false,
        }
    }

    fn clamp(&mut self) {
        self.buffer.col = self.buffer.col.min(
            self.buffer.lines[self.buffer.row]
                .chars()
                .count()
                .saturating_sub(1),
        );
    }

    fn reset_command(&mut self) {
        self.count = 0;
        self.op = None;
        self.pending = None;
        self.register = '"';
        self.register_pending = false;
    }

    fn finish(&mut self) {
        if self.mode == Mode::Normal
            && self.op.is_none()
            && self.pending.is_none()
            && self.prompt.is_none()
            && self.confirmation.is_none()
            && self.count == 0
            && !self.register_pending
        {
            let changed = self.buffer.end_change();
            if !self.replaying {
                if changed {
                    self.last_change = self.recording.clone();
                }
                self.recording.clear();
            }
            self.register = '"';
            self.clamp();
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::ALT) {
            return false;
        }
        if self.confirmation.is_some() {
            if !self.replaying {
                self.recording.push(Input::Key(key));
            }
            self.confirm_key(key.code);
            self.finish();
            return false;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('q') {
            if !self.buffer.dirty() || self.discard_armed {
                return true;
            }
            self.discard_armed = true;
            self.message = "Unsaved changes. Ctrl-Q again discards them.".to_owned();
            return false;
        }
        self.discard_armed = false;
        self.message.clear();
        if ctrl && key.code == KeyCode::Char('s') {
            self.save();
            return false;
        }
        if ctrl && matches!(key.code, KeyCode::Char('z' | 'r')) && self.prompt.is_none() {
            self.leave_insert();
            self.mode = Mode::Normal;
            self.reset_command();
            self.recording.clear();
            if key.code == KeyCode::Char('r') {
                self.buffer.redo();
            } else {
                self.buffer.undo();
            }
            self.clamp();
            return false;
        }
        if self.prompt.is_some() {
            if !self.replaying {
                self.recording.push(Input::Key(key));
            }
            return self.prompt_key(key);
        }
        self.message.clear();
        if !self.replaying {
            self.recording.push(Input::Key(key));
        }
        if key.code == KeyCode::Esc || (ctrl && key.code == KeyCode::Char('[')) {
            if self.mode == Mode::Insert {
                self.leave_insert();
            }
            self.mode = Mode::Normal;
            self.reset_command();
            self.finish();
            return false;
        }
        if ctrl {
            match key.code {
                KeyCode::Char('v') if self.mode != Mode::Insert => self.visual(Visual::Block),
                KeyCode::Char('d' | 'u' | 'f' | 'b') => {
                    let (forward, half) = match key.code {
                        KeyCode::Char('d') => (true, true),
                        KeyCode::Char('u') => (false, true),
                        KeyCode::Char('f') => (true, false),
                        _ => (false, false),
                    };
                    let rows = if half {
                        self.page_rows / 2
                    } else {
                        self.page_rows
                    };
                    self.buffer.move_cursor(
                        0,
                        if forward {
                            rows as isize
                        } else {
                            -(rows as isize)
                        },
                    );
                    self.reset_command();
                }
                _ => {}
            }
        } else if self.mode == Mode::Insert {
            self.insert_key(key.code);
        } else {
            self.normal_key(key.code);
        }
        if self.mode != Mode::Insert {
            self.clamp();
        }
        self.finish();
        false
    }

    pub fn paste(&mut self, text: &str) {
        if self.confirmation.is_some() {
            return;
        }
        if let Some(p) = &mut self.prompt {
            if !self.replaying {
                self.recording.push(Input::Paste(text.to_owned()));
            }
            p.text.extend(text.chars().filter(|ch| !ch.is_control()));
            return;
        }
        self.discard_armed = false;
        let normal = self.mode != Mode::Insert;
        if normal {
            self.reset_command();
            self.recording.clear();
            if matches!(self.mode, Mode::Visual(_)) {
                self.visual_operator('c');
            } else {
                self.enter_insert(1);
            }
        }
        if !self.replaying {
            self.recording.push(Input::Paste(text.to_owned()));
        }
        self.buffer.insert(text);
        self.inserted.push_str(text);
        if normal {
            self.leave_insert();
            self.mode = Mode::Normal;
            self.finish();
        }
    }

    fn enter_insert(&mut self, count: usize) {
        self.buffer.begin_change();
        self.mode = Mode::Insert;
        self.insert_count = count;
        self.inserted.clear();
    }

    fn leave_insert(&mut self) {
        if self.mode != Mode::Insert {
            return;
        }
        if self.insert_count > 1 && !self.inserted.is_empty() {
            self.buffer
                .insert(&self.inserted.repeat(self.insert_count - 1));
        }
        if let Some((start, end, col)) = self.block_insert.take() {
            if !self.inserted.contains('\n') {
                let text = self.inserted.clone();
                for row in start + 1..=end {
                    self.buffer.row = row;
                    self.buffer.col = col.min(self.buffer.lines[row].chars().count());
                    self.buffer.insert(&text);
                }
                self.buffer.row = start;
                self.buffer.col = col + text.chars().count();
            }
        }
        self.buffer.col = self.buffer.col.saturating_sub(1);
        self.insert_count = 1;
        self.inserted.clear();
        self.goal = None;
    }

    fn insert_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(ch) => {
                self.buffer.insert(&ch.to_string());
                self.inserted.push(ch);
            }
            KeyCode::Enter => {
                self.buffer.insert("\n");
                self.inserted.push('\n');
            }
            KeyCode::Tab => {
                self.buffer.insert("\t");
                self.inserted.push('\t');
            }
            KeyCode::Backspace => {
                if self.buffer.col == 0 && self.buffer.row > 0 {
                    self.block_insert = None;
                    self.insert_count = 1;
                    self.inserted.clear();
                }
                self.buffer.backspace();
                self.inserted.pop();
            }
            KeyCode::Delete => {
                self.buffer.delete();
                self.insert_count = 1;
                self.block_insert = None;
            }
            _ => {
                self.arrow(code);
                self.insert_count = 1;
                self.block_insert = None;
                self.inserted.clear();
            }
        }
    }

    fn arrow(&mut self, code: KeyCode) {
        match code {
            KeyCode::Left => self.buffer.move_cursor(-1, 0),
            KeyCode::Right => self.buffer.move_cursor(1, 0),
            KeyCode::Up => self.buffer.move_cursor(0, -1),
            KeyCode::Down => self.buffer.move_cursor(0, 1),
            KeyCode::Home => self.buffer.col = 0,
            KeyCode::End => self.buffer.col = self.buffer.lines[self.buffer.row].chars().count(),
            KeyCode::PageUp => self.buffer.move_cursor(0, -(self.page_rows as isize)),
            KeyCode::PageDown => self.buffer.move_cursor(0, self.page_rows as isize),
            _ => {}
        }
    }

    fn visual(&mut self, kind: Visual) {
        if self.mode == Mode::Visual(kind) {
            self.mode = Mode::Normal;
        } else {
            if !matches!(self.mode, Mode::Visual(_)) {
                self.anchor = (self.buffer.row, self.buffer.col);
            }
            self.mode = Mode::Visual(kind);
        }
        self.reset_command();
    }

    fn normal_key(&mut self, code: KeyCode) {
        let ch = match code {
            KeyCode::Char(ch) => ch,
            KeyCode::Left => 'h',
            KeyCode::Right => 'l',
            KeyCode::Up => 'k',
            KeyCode::Down => 'j',
            KeyCode::Home => '0',
            KeyCode::End => '$',
            KeyCode::Enter => '+',
            KeyCode::Backspace => 'h',
            KeyCode::Delete => 'x',
            _ => {
                self.arrow(code);
                self.reset_command();
                return;
            }
        };
        if !(matches!(ch, 'j' | 'k' | '$')
            || (ch.is_ascii_digit() && (ch != '0' || self.count > 0)))
        {
            self.goal = None;
        }
        if let Some(pending) = self.pending.take() {
            match pending {
                Pending::Register => {
                    if ch.is_ascii_alphanumeric() || matches!(ch, '"' | '_' | '-') {
                        self.register = ch;
                        self.register_pending = true;
                    } else {
                        self.reset_command();
                    }
                }
                Pending::G => {
                    if ch == 'g' {
                        self.apply_motion('g');
                    } else {
                        self.reset_command();
                    }
                }
                Pending::Find(kind) => {
                    let count = self.total_count();
                    self.find = Some((kind, ch));
                    if let Some(m) = motion::find(&self.buffer, kind, ch, count) {
                        self.move_or_operate(m);
                    } else {
                        self.reset_command();
                    }
                }
                Pending::Object(around) => {
                    if let Some((start, end)) =
                        motion::object(&self.buffer, ch, around, self.total_count())
                    {
                        if let Some((op, _, _)) = self.op {
                            self.operate(op, start, end);
                            self.reset_command();
                        } else {
                            self.buffer.set_offset(start);
                            self.anchor = (self.buffer.row, self.buffer.col);
                            self.buffer.set_offset(end.saturating_sub(1));
                        }
                    } else {
                        self.reset_command();
                    }
                }
                Pending::Replace => {
                    let count = self.count.max(1);
                    let start = self.buffer.offset();
                    let length = self.buffer.lines[self.buffer.row].chars().count();
                    if self.buffer.col + count <= length {
                        self.buffer.begin_change();
                        self.buffer
                            .replace(start, start + count, &ch.to_string().repeat(count));
                        self.buffer.set_offset(start + count - 1);
                    }
                    self.reset_command();
                }
            }
            return;
        }
        if ch.is_ascii_digit() && (ch != '0' || self.count > 0) {
            self.count = self
                .count
                .saturating_mul(10)
                .saturating_add(ch.to_digit(10).unwrap() as usize)
                .min(1_000_000);
            return;
        }
        if ch == '"' {
            self.pending = Some(Pending::Register);
            return;
        }
        if ch == 'g' {
            self.pending = Some(Pending::G);
            return;
        }
        if matches!(ch, 'f' | 'F' | 't' | 'T') {
            self.pending = Some(Pending::Find(ch));
            return;
        }
        if (self.op.is_some() || matches!(self.mode, Mode::Visual(_))) && matches!(ch, 'i' | 'a') {
            self.pending = Some(Pending::Object(ch == 'a'));
            return;
        }
        if matches!(ch, ';' | ',') {
            if let Some((mut kind, target)) = self.find {
                if ch == ',' {
                    kind = match kind {
                        'f' => 'F',
                        'F' => 'f',
                        't' => 'T',
                        _ => 't',
                    };
                }
                // Till repeats must skip the target immediately adjacent to the cursor.
                let original = self.buffer.col;
                if matches!(kind, 't' | 'T') {
                    self.buffer.move_cursor(if kind == 't' { 1 } else { -1 }, 0);
                }
                let m = motion::find(&self.buffer, kind, target, self.total_count());
                self.buffer.col = original;
                if let Some(m) = m {
                    self.move_or_operate(m);
                } else {
                    self.reset_command();
                }
            } else {
                self.reset_command();
            }
            return;
        }
        if matches!(self.mode, Mode::Visual(_)) {
            match ch {
                'p' | 'P' => {
                    if let Some(value) = self
                        .registers
                        .get(&self.register.to_ascii_lowercase())
                        .cloned()
                    {
                        let count = self.count.max(1);
                        self.visual_operator('d');
                        self.put_value(value, false, count);
                    }
                    return;
                }
                'd' | 'x' | 'c' | 's' | 'y' | '>' | '<' => {
                    self.visual_operator(match ch {
                        'x' => 'd',
                        's' => 'c',
                        _ => ch,
                    });
                    return;
                }
                'o' => {
                    std::mem::swap(&mut self.anchor.0, &mut self.buffer.row);
                    std::mem::swap(&mut self.anchor.1, &mut self.buffer.col);
                    return;
                }
                'v' => {
                    self.visual(Visual::Character);
                    return;
                }
                'V' => {
                    self.visual(Visual::Line);
                    return;
                }
                _ => {}
            }
        }
        if matches!(ch, ':' | '/' | '?') {
            if ch == ':' && matches!(self.mode, Mode::Visual(_)) {
                self.visual_range = Some((
                    self.anchor.0.min(self.buffer.row),
                    self.anchor.0.max(self.buffer.row),
                ));
                self.mode = Mode::Normal;
                self.prompt = Some(Prompt {
                    kind: ch,
                    text: "'<,'>".into(),
                });
            } else {
                self.prompt = Some(Prompt {
                    kind: ch,
                    text: String::new(),
                });
            }
            return;
        }
        if matches!(ch, 'n' | 'N') {
            self.repeat_search(ch == 'N');
            return;
        }
        if matches!(ch, '*' | '#') {
            self.keyword_search(ch == '*');
            return;
        }
        if matches!(self.mode, Mode::Visual(_)) {
            if motion::motion(&self.buffer, ch, self.total_count(), self.explicit_count()).is_some()
            {
                self.apply_motion(ch);
                return;
            }
            // Normal-only commands end the selection before changing its buffer.
            self.mode = Mode::Normal;
            self.reset_command();
        }
        if let Some((op, op_count, _)) = self.op {
            if ch == op {
                let start = self.buffer.row;
                let end = (start + self.count.max(1).saturating_mul(op_count))
                    .min(self.buffer.lines.len());
                self.line_operator(op, start, end);
                self.reset_command();
                return;
            }
            if op == 'c'
                && matches!(ch, 'w' | 'W')
                && self.buffer.lines[self.buffer.row]
                    .chars()
                    .nth(self.buffer.col)
                    .is_some_and(|c| !c.is_whitespace())
            {
                self.apply_motion(if ch == 'w' { 'e' } else { 'E' });
                return;
            }
            if motion::motion(&self.buffer, ch, self.total_count(), self.explicit_count()).is_some()
            {
                self.apply_motion(ch);
            } else {
                self.reset_command();
            }
            return;
        }
        let count = self.count.max(1);
        match ch {
            'i' | 'a' | 'I' | 'A' => {
                match ch {
                    'a' => {
                        self.buffer.col = (self.buffer.col + 1)
                            .min(self.buffer.lines[self.buffer.row].chars().count())
                    }
                    'I' => {
                        self.buffer.col =
                            motion::first_nonblank(&self.buffer.lines[self.buffer.row])
                    }
                    'A' => self.buffer.col = self.buffer.lines[self.buffer.row].chars().count(),
                    _ => {}
                }
                self.enter_insert(count);
                self.count = 0;
            }
            'o' | 'O' => {
                self.buffer.begin_change();
                let row = self.buffer.row + usize::from(ch == 'o');
                self.buffer.replace_lines(row, row, &[String::new()]);
                self.enter_insert(count);
                self.count = 0;
            }
            'd' | 'c' | 'y' | '>' | '<' => {
                self.op = Some((ch, count, self.count > 0));
                self.count = 0;
            }
            'x' | 'X' | 's' => {
                let offset = self.buffer.offset();
                let col = self.buffer.col;
                let (start, end) = if ch == 'X' {
                    (offset - col.min(count), offset)
                } else {
                    (
                        offset,
                        offset
                            + (self.buffer.lines[self.buffer.row].chars().count() - col).min(count),
                    )
                };
                if end > start {
                    self.operate(if ch == 's' { 'c' } else { 'd' }, start, end);
                }
                self.reset_command();
            }
            'D' | 'C' => {
                let start = self.buffer.offset();
                let row = (self.buffer.row + count - 1).min(self.buffer.lines.len() - 1);
                let end = self
                    .buffer
                    .offset_at(row, self.buffer.lines[row].chars().count());
                self.operate(if ch == 'D' { 'd' } else { 'c' }, start, end);
                self.reset_command();
            }
            'S' => {
                let row = self.buffer.row;
                self.line_operator('c', row, (row + count).min(self.buffer.lines.len()));
                self.reset_command();
            }
            'Y' => {
                let row = self.buffer.row;
                self.line_operator('y', row, (row + count).min(self.buffer.lines.len()));
                self.reset_command();
            }
            'p' | 'P' => {
                self.put(ch == 'p', count);
                self.reset_command();
            }
            'u' => {
                for _ in 0..count.min(self.buffer.lines.len() + 10000) {
                    self.buffer.undo();
                }
                self.reset_command();
            }
            '.' => {
                let change = self.last_change.clone();
                self.reset_command();
                self.recording.clear();
                if !change.is_empty() {
                    self.replaying = true;
                    for _ in 0..count {
                        for input in &change {
                            match input {
                                Input::Key(key) => {
                                    self.key(*key);
                                }
                                Input::Paste(text) => self.paste(text),
                            }
                        }
                    }
                    self.replaying = false;
                }
            }
            'v' => self.visual(Visual::Character),
            'V' => self.visual(Visual::Line),
            'r' => self.pending = Some(Pending::Replace),
            'J' => {
                self.buffer.begin_change();
                let start = self.buffer.row;
                for _ in 0..count.max(2) - 1 {
                    if start + 1 >= self.buffer.lines.len() {
                        break;
                    }
                    let left = self.buffer.lines[start].trim_end().to_owned();
                    let right = self.buffer.lines[start + 1].trim_start();
                    let col = left.chars().count();
                    let separator = if left.is_empty() || right.is_empty() {
                        ""
                    } else {
                        " "
                    };
                    let line = format!("{left}{separator}{right}");
                    self.buffer.replace_lines(start, start + 2, &[line]);
                    self.buffer.col = col;
                }
                self.reset_command();
            }
            _ => {
                self.apply_motion(ch);
            }
        }
    }

    fn total_count(&self) -> usize {
        self.count
            .max(1)
            .saturating_mul(self.op.map_or(1, |(_, c, _)| c))
            .min(1_000_000)
    }

    fn explicit_count(&self) -> bool {
        self.count > 0 || self.op.is_some_and(|(_, _, explicit)| explicit)
    }

    fn apply_motion(&mut self, ch: char) {
        if let Some(mut m) =
            motion::motion(&self.buffer, ch, self.total_count(), self.explicit_count())
        {
            if matches!(ch, 'j' | 'k') && self.op.is_none() {
                let goal = self.goal.unwrap_or(self.buffer.col);
                let original = (self.buffer.row, self.buffer.col);
                self.buffer.set_offset(m.offset);
                m.offset = self.buffer.offset_at(
                    self.buffer.row,
                    goal.min(
                        self.buffer.lines[self.buffer.row]
                            .chars()
                            .count()
                            .saturating_sub(1),
                    ),
                );
                (self.buffer.row, self.buffer.col) = original;
                self.goal = Some(goal);
            } else if ch == '$' {
                self.goal = Some(usize::MAX);
            }
            self.move_or_operate(m);
        } else {
            self.reset_command();
        }
    }

    fn move_or_operate(&mut self, m: Motion) {
        if let Some((op, _, _)) = self.op {
            if m.linewise {
                let row = self.buffer.row;
                self.buffer.set_offset(m.offset);
                let target = self.buffer.row;
                self.buffer.row = row;
                self.line_operator(op, row.min(target), row.max(target) + 1);
            } else {
                let pos = self.buffer.offset();
                let start = pos.min(m.offset);
                let mut end = pos.max(m.offset) + usize::from(m.inclusive);
                // Exclusive motions landing at column zero stop before that line.
                if !m.inclusive && m.offset > pos {
                    let row = self.buffer.row;
                    let col = self.buffer.col;
                    self.buffer.set_offset(m.offset);
                    if self.buffer.col == 0 && self.buffer.row > row {
                        end = end.saturating_sub(1);
                    }
                    self.buffer.row = row;
                    self.buffer.col = col;
                }
                self.operate(op, start, end);
            }
        } else {
            self.buffer.set_offset(m.offset);
        }
        self.reset_command();
    }

    fn remember(&mut self, text: String, kind: Visual, op: char) {
        if self.register == '_' {
            return;
        }
        let mut value = Register { text, kind };
        let target = self.register.to_ascii_lowercase();
        if self.register.is_ascii_uppercase() {
            if let Some(old) = self.registers.get(&target) {
                value.text = format!("{}{}", old.text, value.text);
            }
        }
        if target != '"' {
            self.registers.insert(target, value.clone());
        }
        self.registers.insert('"', value.clone());
        if op == 'y' {
            self.registers.insert('0', value);
        } else {
            for n in (2..=9).rev() {
                if let Some(previous) = self.registers.get(&char::from(b'0' + n - 1)).cloned() {
                    self.registers.insert(char::from(b'0' + n), previous);
                }
            }
            self.registers.insert('1', value);
        }
    }

    fn operate(&mut self, op: char, start: usize, end: usize) {
        if matches!(op, '>' | '<') {
            self.buffer.set_offset(start);
            let first = self.buffer.row;
            self.buffer.set_offset(end.saturating_sub(1).max(start));
            let last = self.buffer.row;
            self.line_operator(op, first, last + 1);
            return;
        }
        let text: String = self
            .buffer
            .body()
            .chars()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect();
        self.remember(text, Visual::Character, op);
        if op != 'y' {
            self.buffer.begin_change();
            self.buffer.replace(start, end, "");
            if op == 'c' {
                self.enter_insert(1);
            }
        }
        self.buffer.set_offset(start);
    }

    fn line_operator(&mut self, op: char, start: usize, end: usize) {
        if matches!(op, '>' | '<') {
            self.buffer.begin_change();
            let lines: Vec<_> = self.buffer.lines[start..end]
                .iter()
                .map(|line| {
                    if op == '>' {
                        format!("    {line}")
                    } else {
                        let n = if line.starts_with('\t') {
                            1
                        } else {
                            line.chars().take(4).take_while(|&c| c == ' ').count()
                        };
                        line.chars().skip(n).collect()
                    }
                })
                .collect();
            self.buffer.replace_lines(start, end, &lines);
            self.buffer.col = motion::first_nonblank(&self.buffer.lines[start]);
            return;
        }
        let text = format!("{}\n", self.buffer.lines[start..end].join("\n"));
        self.remember(text, Visual::Line, op);
        if op != 'y' {
            self.buffer.begin_change();
            let replacement = if op == 'c' {
                vec![String::new()]
            } else {
                Vec::new()
            };
            self.buffer.replace_lines(start, end, &replacement);
            if op == 'c' {
                self.enter_insert(1);
            }
        } else {
            self.buffer.row = start;
            self.buffer.col = 0;
        }
    }

    fn visual_operator(&mut self, op: char) {
        let Mode::Visual(kind) = self.mode else {
            return;
        };
        let (ar, ac) = self.anchor;
        let (br, bc) = (self.buffer.row, self.buffer.col);
        self.mode = Mode::Normal;
        match kind {
            Visual::Line => self.line_operator(op, ar.min(br), ar.max(br) + 1),
            Visual::Character => {
                let a = self.buffer.offset_at(ar, ac);
                let b = self.buffer.offset();
                if matches!(op, '>' | '<') {
                    self.line_operator(op, ar.min(br), ar.max(br) + 1);
                } else {
                    self.operate(op, a.min(b), a.max(b) + 1);
                }
            }
            Visual::Block => {
                let start = ar.min(br);
                let end = ar.max(br);
                let col = ac.min(bc);
                let width = ac.max(bc) - col + 1;
                let text = self.buffer.lines[start..=end]
                    .iter()
                    .map(|l| l.chars().skip(col).take(width).collect::<String>())
                    .collect::<Vec<_>>()
                    .join("\n");
                if matches!(op, '>' | '<') {
                    self.line_operator(op, start, end + 1);
                } else {
                    self.remember(text, Visual::Block, op);
                    if op != 'y' {
                        self.buffer.begin_change();
                        for row in (start..=end).rev() {
                            let a = self.buffer.offset_at(row, col);
                            let b = self.buffer.offset_at(row, col + width);
                            self.buffer.replace(a, b, "");
                        }
                        if op == 'c' {
                            self.enter_insert(1);
                            self.block_insert = Some((start, end, col));
                        }
                    }
                    self.buffer.row = start;
                    self.buffer.col = col.min(self.buffer.lines[start].chars().count());
                }
            }
        }
        self.reset_command();
    }

    fn put(&mut self, after: bool, count: usize) {
        let Some(value) = self
            .registers
            .get(&self.register.to_ascii_lowercase())
            .cloned()
        else {
            return;
        };
        self.put_value(value, after, count);
    }

    fn put_value(&mut self, value: Register, after: bool, count: usize) {
        self.buffer.begin_change();
        match value.kind {
            Visual::Line => {
                let text = value.text.repeat(count);
                let lines: Vec<_> = text
                    .strip_suffix('\n')
                    .unwrap_or(&text)
                    .split('\n')
                    .map(str::to_owned)
                    .collect();
                let row = self.buffer.row + usize::from(after);
                self.buffer.put_lines(row, &lines);
            }
            Visual::Character => {
                let pos = self.buffer.offset()
                    + usize::from(after && !self.buffer.lines[self.buffer.row].is_empty());
                self.buffer.replace(pos, pos, &value.text.repeat(count));
                self.buffer.col = self.buffer.col.saturating_sub(1);
            }
            Visual::Block => {
                let row = self.buffer.row;
                let col = self.buffer.col + usize::from(after);
                for (n, line) in value.text.split('\n').enumerate() {
                    if row + n >= self.buffer.lines.len() {
                        self.buffer
                            .replace_lines(row + n, row + n, &[String::new()]);
                    }
                    self.buffer.row = row + n;
                    self.buffer.col = self.buffer.lines[row + n].chars().count();
                    if self.buffer.col < col {
                        self.buffer.insert(&" ".repeat(col - self.buffer.col));
                    }
                    self.buffer.col = col;
                    self.buffer.insert(&line.repeat(count));
                }
                self.buffer.row = row;
                self.buffer.col = col;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> Editor {
        Editor::new(Buffer::from_text(text))
    }
    fn keys(e: &mut Editor, text: &str) {
        for ch in text.chars() {
            let code = match ch {
                '\u{1b}' => KeyCode::Esc,
                '\n' => KeyCode::Enter,
                _ => KeyCode::Char(ch),
            };
            e.key(KeyEvent::new(code, KeyModifiers::NONE));
        }
    }
    fn ctrl(e: &mut Editor, ch: char) {
        e.key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL));
    }

    #[test]
    fn search_directions_wrap_word_search_and_operator_search() {
        let mut e = editor("one two one\nstone one\n");
        keys(&mut e, "/one\n");
        assert_eq!((e.buffer.row, e.buffer.col), (0, 8));
        keys(&mut e, "n");
        assert_eq!((e.buffer.row, e.buffer.col), (1, 2));
        keys(&mut e, "N");
        assert_eq!((e.buffer.row, e.buffer.col), (0, 8));
        keys(&mut e, "?two\n");
        assert_eq!(e.buffer.col, 4);
        keys(&mut e, "gg*");
        assert_eq!(e.buffer.col, 8);
        keys(&mut e, "n");
        assert_eq!((e.buffer.row, e.buffer.col), (1, 6));
        keys(&mut e, "ggd/two\n");
        assert_eq!(e.buffer.text(), "two one\nstone one\n");
        keys(&mut e, "/missing\n");
        assert!(e.message.contains("Pattern not found"));
    }

    #[test]
    fn substitutions_ranges_flags_and_capture_expansion() {
        let mut e = editor("one ONE one\none two\nlast\n");
        keys(&mut e, ":%s/one/X/gi\n");
        assert_eq!(e.buffer.text(), "X X X\nX two\nlast\n");
        keys(&mut e, "u");
        assert_eq!(e.buffer.text(), "one ONE one\none two\nlast\n");
        keys(&mut e, ":2,3s/\\(one\\) \\(two\\)/\\2-\\1/\n");
        assert_eq!(e.buffer.text(), "one ONE one\ntwo-one\nlast\n");
        keys(&mut e, ":1s/one/[&]/\n");
        assert_eq!(e.buffer.lines[0], "[one] ONE one");
        keys(&mut e, ":%s/[bad/x/\n");
        assert!(e.message.contains("Invalid pattern"));
    }

    #[test]
    fn substitution_confirmation_yes_no_all_last_quit_and_undo() {
        let mut e = editor("one one\none\n");
        keys(&mut e, ":%s/one/X/gc\n");
        assert!(e.message.contains("replace with"));
        keys(&mut e, "nyl");
        assert_eq!(e.buffer.text(), "one X\nX\n");
        keys(&mut e, "u");
        assert_eq!(e.buffer.text(), "one one\none\n");
        keys(&mut e, ":%s/one/Y/gc\na");
        assert_eq!(e.buffer.text(), "Y Y\nY\n");
        keys(&mut e, "u:%s/one/Y/gc\nq");
        assert_eq!(e.buffer.text(), "one one\none\n");
    }

    #[test]
    fn ex_addresses_errors_and_dirty_quit_protection() {
        let mut e = editor("one\ntwo\nthree\n");
        keys(&mut e, ":$\n");
        assert_eq!(e.buffer.row, 2);
        keys(&mut e, ":1\n");
        assert_eq!(e.buffer.row, 0);
        keys(&mut e, ":2,3d\n");
        assert_eq!(e.buffer.text(), "one\n");
        keys(&mut e, ":q\n");
        assert!(e.message.contains("Unsaved changes"));
        keys(&mut e, ":999s/a/b/\n");
        assert!(e.message.contains("range"));
        keys(&mut e, ":%s/a/b/z\n");
        assert!(e.message.contains("flag"));
        keys(&mut e, ":q!");
        assert!(e.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    }

    #[test]
    fn visual_paste_replaces_selection_and_block_change_repeats_insert() {
        let mut e = editor("abcd\nefgh\nijkl");
        keys(&mut e, "yl0vllp");
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(e.buffer.lines[0], "ad");
        keys(&mut e, "u0l");
        ctrl(&mut e, 'v');
        keys(&mut e, "jlcX\x1b");
        assert_eq!(e.buffer.text(), "aXd\neXh\nijkl");
        keys(&mut e, "u");
        assert_eq!(e.buffer.text(), "abcd\nefgh\nijkl");
    }

    #[test]
    fn vertical_motions_keep_the_desired_column() {
        let mut e = editor("abcdef\nx\nabcdef");
        keys(&mut e, "4ljj");
        assert_eq!((e.buffer.row, e.buffer.col), (2, 4));
        keys(&mut e, "$kk");
        assert_eq!((e.buffer.row, e.buffer.col), (0, 5));
        keys(&mut e, "0jj");
        assert_eq!(e.buffer.col, 0);
    }

    #[test]
    fn till_repeats_and_exclusive_line_crossing() {
        let mut e = editor("abc xyz xyz\nlast");
        keys(&mut e, "tx;");
        assert_eq!(e.buffer.col, 7);
        keys(&mut e, ",");
        assert_eq!(e.buffer.col, 5);
        keys(&mut e, "gg0$dw");
        assert_eq!(e.buffer.text(), "abc xyz xy\nlast");
    }

    #[test]
    fn substitute_repeat_cancel_paste_and_visual_ranges() {
        let mut e = editor("one one\none\nlast");
        keys(&mut e, ":s/one/X/\n.");
        assert_eq!(e.buffer.lines[0], "X X");
        keys(&mut e, "u");
        assert_eq!(e.buffer.lines[0], "X one");
        keys(&mut e, "ggVj:s/one/Y/g\n");
        assert_eq!(e.buffer.text(), "X Y\nY\nlast");
        keys(&mut e, "/abandoned\x1b");
        assert!(e.prompt.is_none());
        keys(&mut e, "i");
        e.paste("é界\n");
        keys(&mut e, "\x1bu");
        assert_eq!(e.buffer.text(), "X Y\nY\nlast");
    }

    #[test]
    fn named_registers_are_independent_and_uppercase_appends() {
        let mut e = editor("one two three");
        keys(&mut e, "\"ayiw w\"byiw w\"aP");
        assert_eq!(e.buffer.text(), "one two onethree");
        keys(&mut e, "u0\"ayiw w\"Ayiw G$\"ap");
        assert_eq!(e.buffer.text(), "one two threeonetwo");
        keys(&mut e, "u0\"_diw w\"aP");
        assert_eq!(e.buffer.text(), " onetwotwo three");
    }

    #[test]
    fn linewise_paste_keeps_blank_lines() {
        let mut e = editor("one\n\nlast\n");
        keys(&mut e, "2yyGp");
        assert_eq!(e.buffer.text(), "one\n\nlast\none\n\n");
    }

    #[test]
    fn file_commands_preserve_unsaved_changes_and_existing_files() {
        let root = std::env::temp_dir().join(format!("fvim-ex-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first file é.txt");
        let second = root.join("second.txt");
        std::fs::write(&second, "existing\r\n").unwrap();
        let mut e = editor("");
        keys(&mut e, "inew\x1b");
        e.execute_ex(&format!("w {}", first.display())).unwrap();
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "new");
        assert!(!e.buffer.dirty());
        keys(&mut e, "a!\x1b");
        assert!(e.execute_ex(&format!("e {}", second.display())).is_err());
        assert!(e.execute_ex(&format!("w {}", second.display())).is_err());
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "existing\r\n");
        e.execute_ex(&format!("e! {}", second.display())).unwrap();
        assert_eq!(e.buffer.text(), "existing\r\n");
        keys(&mut e, "0cwOTHER\x1b");
        assert!(e.execute_ex("wq").unwrap());
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "OTHER\r\n");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn history_keys_clear_stale_save_feedback() {
        let mut e = editor("one");
        keys(&mut e, "ix\x1bu");
        e.message = "Saved.".into();
        ctrl(&mut e, 'r');
        assert!(e.message.is_empty());
        assert_eq!(e.buffer.text(), "xone");
    }

    #[test]
    fn indentation_motions_and_objects_preserve_text() {
        let mut e = editor("one two\nlast\n");
        keys(&mut e, ">w");
        assert_eq!(e.buffer.text(), "    one two\nlast\n");
        keys(&mut e, "<iw");
        assert_eq!(e.buffer.text(), "one two\nlast\n");
    }

    #[test]
    fn visual_history_and_join_leave_valid_selection_state() {
        let mut e = editor("one");
        keys(&mut e, "oTWO\x1bvud\x1b");
        assert_eq!(e.buffer.text(), "one");
        assert_eq!(e.mode, Mode::Normal);
        let mut e = editor("one\ntwo\nthree");
        keys(&mut e, "GvkkJd\x1b");
        assert_eq!(e.buffer.text(), "one two\nthree");
    }

    #[test]
    fn block_insert_cancels_replication_on_row_merge() {
        let mut e = editor("abc\ndef\nghi");
        keys(&mut e, "j0");
        ctrl(&mut e, 'v');
        keys(&mut e, "jc");
        e.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        keys(&mut e, "X\x1b");
        assert_eq!(e.buffer.text(), "abcXef\nhi");
        keys(&mut e, "u");
        assert_eq!(e.buffer.text(), "abc\ndef\nghi");
    }

    #[test]
    fn confirmation_ignores_paste_and_prompt_paste_is_repeatable() {
        let mut e = editor("one two one");
        keys(&mut e, ":%s/one/X/gc\n");
        e.paste("hello");
        keys(&mut e, "a");
        assert_eq!(e.buffer.text(), "X two X");
        let mut e = editor("one one");
        keys(&mut e, ":");
        e.paste("s/one/X/");
        keys(&mut e, "\n.");
        assert_eq!(e.buffer.text(), "X X");
    }

    #[test]
    fn explicit_operator_counts_control_absolute_destinations() {
        let text = "one\ntwo\nthree\nfour";
        for (command, result) in [
            ("2dG", "three\nfour"),
            ("1dG", "two\nthree\nfour"),
            ("G2dgg", "one"),
            ("50d%", "three\nfour"),
        ] {
            let mut e = editor(text);
            keys(&mut e, command);
            assert_eq!(e.buffer.text(), result, "{command}");
        }
    }

    #[test]
    fn operators_accept_search_repeat_and_keyword_motions() {
        let mut e = editor("one two one");
        keys(&mut e, "/one\nggdn");
        assert_eq!(e.buffer.text(), "one");
        let mut e = editor("one two one");
        keys(&mut e, "d*");
        assert_eq!(e.buffer.text(), "one");
        keys(&mut e, "u$d#");
        assert_eq!(e.buffer.text(), "e");
    }

    #[test]
    fn writing_an_alternate_copy_keeps_original_dirty() {
        let root = std::env::temp_dir().join(format!("fvim-copy-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        std::fs::write(&a, "old").unwrap();
        let mut e = Editor::new(Buffer::open(Some(a.clone())).unwrap());
        keys(&mut e, "inew\x1b");
        e.execute_ex(&format!("w {}", b.display())).unwrap();
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "newold");
        assert!(e.buffer.dirty());
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "old");
        assert!(e.execute_ex("e").is_err());
        assert!(e.execute_ex("x").unwrap());
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "newold");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normal_insert_transactions_redo_and_dot() {
        let mut e = editor("one two");
        assert_eq!(e.mode, Mode::Normal);
        keys(&mut e, "ihello \x1b");
        assert_eq!(e.buffer.text(), "hello one two");
        keys(&mut e, "u");
        assert_eq!(e.buffer.text(), "one two");
        ctrl(&mut e, 'r');
        assert_eq!(e.buffer.text(), "hello one two");
        keys(&mut e, "w.");
        assert_eq!(e.buffer.text(), "hello hello one two");
    }

    #[test]
    fn operator_counts_and_linewise_registers() {
        let mut e = editor("one two three four five six seven\nlast\n");
        keys(&mut e, "2d3w");
        assert_eq!(e.buffer.text(), "seven\nlast\n");
        keys(&mut e, "u2ddp");
        assert_eq!(e.buffer.text(), "one two three four five six seven\nlast\n");
        keys(&mut e, "ggyyGp");
        assert_eq!(
            e.buffer.text(),
            "one two three four five six seven\nlast\none two three four five six seven\n"
        );
    }

    #[test]
    fn changes_text_objects_named_registers_and_blackhole() {
        let mut e = editor("one two (inner)\nlast");
        keys(&mut e, "cwONE\x1b");
        // Move into the pair independently; cw must leave its separating space.
        assert_eq!(e.buffer.text(), "ONE two (inner)\nlast");
        keys(&mut e, "f(ci(TWO\x1b");
        assert_eq!(e.buffer.text(), "ONE two (TWO)\nlast");
        keys(&mut e, "gg\"ayiw\"_diw\"aP");
        assert_eq!(e.buffer.text(), "ONE two (TWO)\nlast");
    }

    #[test]
    fn visual_character_line_and_block_operations() {
        let mut e = editor("abcd\nefgh\nijkl");
        keys(&mut e, "vld");
        assert_eq!(e.buffer.text(), "cd\nefgh\nijkl");
        keys(&mut e, "uVjyGp");
        assert_eq!(e.buffer.text(), "abcd\nefgh\nijkl\nabcd\nefgh");
        keys(&mut e, "ggl");
        ctrl(&mut e, 'v');
        keys(&mut e, "jld");
        assert_eq!(e.buffer.text(), "ad\neh\nijkl\nabcd\nefgh");
        keys(&mut e, "u");
        assert_eq!(e.buffer.text(), "abcd\nefgh\nijkl\nabcd\nefgh");
    }

    #[test]
    fn find_repeat_join_indent_replace_and_open_lines() {
        let mut e = editor("abc xyz xyz\n  second\n");
        keys(&mut e, "fx;");
        assert_eq!(e.buffer.col, 8);
        keys(&mut e, ",");
        assert_eq!(e.buffer.col, 4);
        keys(&mut e, "0dfx");
        assert_eq!(e.buffer.text(), "yz xyz\n  second\n");
        keys(&mut e, "uJ");
        assert_eq!(e.buffer.text(), "abc xyz xyz second\n");
        keys(&mut e, ">>0rX");
        assert_eq!(e.buffer.text(), "X   abc xyz xyz second\n");
        keys(&mut e, "oend\x1b");
        assert_eq!(e.buffer.text(), "X   abc xyz xyz second\nend\n");
    }

    #[test]
    fn cancelled_operator_and_empty_buffer_are_safe() {
        let mut e = editor("");
        keys(&mut e, "d\x1bxp%999999999999999999999j");
        assert_eq!(e.buffer.text(), "");
        assert_eq!(e.mode, Mode::Normal);
        keys(&mut e, "i界é\x1b0x");
        assert_eq!(e.buffer.text(), "é");
    }
}
