mod buffer;
mod install;

use buffer::Buffer;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute, Print, ResetColor, SetAttribute},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use unicode_width::UnicodeWidthChar;

struct Terminal;

impl Terminal {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste, Hide)?;
        Ok(guard)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), ResetColor, SetAttribute(Attribute::Reset), Show, DisableBracketedPaste, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

fn char_width(ch: char, column: usize) -> usize {
    if ch == '\t' {
        4 - column % 4
    } else if ch.is_control() {
        1
    } else {
        ch.width().unwrap_or(1)
    }
}

fn display_column(line: &str, chars: usize) -> usize {
    line.chars().take(chars).fold(0, |col, ch| col + char_width(ch, col))
}

fn visible_line(line: &str, left: usize, width: usize) -> String {
    let mut output = String::new();
    let mut column = 0;
    for ch in line.chars() {
        let size = char_width(ch, column);
        let end = column + size;
        if size == 0 {
            if column > left && column <= left + width {
                output.push(ch);
            }
        } else if column >= left && end <= left + width {
            if ch == '\t' {
                output.push_str(&" ".repeat(size));
            } else if ch.is_control() {
                output.push('?');
            } else {
                output.push(ch);
            }
        } else if end > left && column < left + width {
            let count = end.min(left + width) - column.max(left);
            output.push_str(&" ".repeat(count));
        }
        column = end;
        if column > left + width {
            break;
        }
    }
    output
}

fn render(buffer: &Buffer, top: &mut usize, left: &mut usize, message: &str) -> io::Result<()> {
    let (width, height) = terminal::size()?;
    if width == 0 || height < 3 {
        return Ok(());
    }
    let rows = usize::from(height - 2);
    if buffer.row < *top {
        *top = buffer.row;
    } else if buffer.row >= *top + rows {
        *top = buffer.row + 1 - rows;
    }
    let cursor = display_column(&buffer.lines[buffer.row], buffer.col);
    let columns = usize::from(width);
    if cursor < *left {
        *left = cursor;
    } else if cursor >= *left + columns {
        *left = cursor + 1 - columns;
    }
    let mut out = io::stdout().lock();
    queue!(out, Hide, MoveTo(0, 0), Clear(ClearType::All))?;
    for screen_row in 0..rows {
        queue!(out, MoveTo(0, screen_row as u16))?;
        if let Some(line) = buffer.lines.get(*top + screen_row) {
            queue!(out, Print(visible_line(line, *left, columns)))?;
        } else {
            queue!(out, Print("~"))?;
        }
    }
    let name = buffer.path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "[No Name]".to_owned());
    let status = format!(" {}{} | {}:{} | Ctrl-S save  Ctrl-Q quit  Ctrl-Z undo",
        name, if buffer.dirty() { " [+]" } else { "" }, buffer.row + 1, buffer.col + 1);
    queue!(out, MoveTo(0, height - 2), SetAttribute(Attribute::Reverse),
        Print(visible_line(&status, 0, columns)), SetAttribute(Attribute::Reset),
        MoveTo(0, height - 1), Print(visible_line(message, 0, columns)),
        MoveTo((cursor - *left) as u16, (buffer.row - *top) as u16), Show)?;
    out.flush()
}

fn edit(path: Option<PathBuf>) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("Interactive editing requires a terminal."));
    }
    let mut buffer = Buffer::open(path)?;
    let _terminal = Terminal::enter()?;
    let (mut top, mut left) = (0, 0);
    let mut message = String::new();
    let mut discard_armed = false;
    loop {
        render(&buffer, &mut top, &mut left, &message)?;
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
                    if !buffer.dirty() || discard_armed {
                        return Ok(());
                    }
                    discard_armed = true;
                    message = "Unsaved changes. Ctrl-Q again discards them.".to_owned();
                    continue;
                }
                discard_armed = false;
                message.clear();
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match key.code {
                        KeyCode::Char('s') => {
                            message = match buffer.save() {
                                Ok(()) => "Saved.".to_owned(),
                                Err(error) => format!("Save failed: {error}"),
                            };
                        }
                        KeyCode::Char('z') => buffer.undo(),
                        _ => {}
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Left => buffer.move_cursor(-1, 0),
                    KeyCode::Right => buffer.move_cursor(1, 0),
                    KeyCode::Up => buffer.move_cursor(0, -1),
                    KeyCode::Down => buffer.move_cursor(0, 1),
                    KeyCode::Home => buffer.col = 0,
                    KeyCode::End => buffer.col = buffer.lines[buffer.row].chars().count(),
                    KeyCode::PageUp => buffer.move_cursor(0, -(terminal::size()?.1.saturating_sub(2) as isize)),
                    KeyCode::PageDown => buffer.move_cursor(0, terminal::size()?.1.saturating_sub(2) as isize),
                    KeyCode::Enter => buffer.insert("\n"),
                    KeyCode::Backspace => buffer.backspace(),
                    KeyCode::Delete => buffer.delete(),
                    KeyCode::Tab => buffer.insert("\t"),
                    KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::ALT) => buffer.insert(&ch.to_string()),
                    _ => {}
                }
            }
            Event::Paste(text) => {
                discard_armed = false;
                buffer.insert(&text);
            }
            _ => {}
        }
    }
}

fn run() -> io::Result<()> {
    let mut args = env::args_os().skip(1);
    let first = args.next();
    match first.as_deref().and_then(|s| s.to_str()) {
        Some("--version") => println!("fvim {}", env!("CARGO_PKG_VERSION")),
        Some("--help") | Some("-h") => println!("fvim [file]\nCtrl-S save | Ctrl-Q quit | Ctrl-Z undo\n--version | --config-path"),
        Some("--config-path") => println!("{}", install::config_dir()?.join("init.lua").display()),
        Some("--install") => install::install()?,
        Some("--init-config") => install::init_config()?,
        Some("--") => return edit(args.next().map(PathBuf::from)),
        Some(arg) if arg.starts_with('-') => return Err(io::Error::other(format!("Unknown option: {arg}"))),
        _ => {
            if args.next().is_some() {
                return Err(io::Error::other("Stage 1 opens one file at a time."));
            }
            return edit(first.map(PathBuf::from));
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fvim: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendering_sanitizes_control_sequences() {
        assert_eq!(visible_line("a\u{1b}[31mb", 0, 40), "a?[31mb");
        assert_eq!(visible_line("a\tb", 0, 8), "a   b");
    }

    #[test]
    fn wide_and_combining_columns() {
        assert_eq!(display_column("a界e\u{301}\t", 5), 8);
        assert_eq!(visible_line("a界b", 2, 2), " b");
        assert_eq!(visible_line("e\u{301}", 0, 1), "e\u{301}");
    }
}
