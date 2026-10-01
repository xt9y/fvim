mod buffer;
mod command;
mod config;
mod diagnostics;
mod editor;
mod install;
mod motion;
#[cfg(test)]
mod performance;
mod picker;
mod renderer;
mod syntax;
mod tooling;
mod workspace;

use buffer::Buffer;
use crossterm::{
    cursor::{Hide, SetCursorStyle, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    style::{Attribute, ResetColor, SetAttribute},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use renderer::Renderer;
use std::env;
use std::io::{self, BufWriter, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use workspace::Workspace;

struct Terminal;

impl Terminal {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            Hide
        )?;
        Ok(guard)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            ResetColor,
            SetCursorStyle::DefaultUserShape,
            SetAttribute(Attribute::Reset),
            Show,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

fn edit(path: Option<PathBuf>) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("Interactive editing requires a terminal."));
    }
    let config_dir = install::config_dir()?;
    let mut config = config::Config::load(&config_dir).map_err(io::Error::other)?;
    let mut workspace = Workspace::new(Buffer::open(path)?, config_dir.clone());
    workspace.apply_settings(config.settings.clone());
    let _terminal = Terminal::enter()?;
    let mut renderer = Renderer::default();
    let mut out =
        BufWriter::with_capacity(workspace.settings.output_buffer_size, io::stdout().lock());
    let mut redraw = true;
    let mut building = false;
    loop {
        if workspace.build.is_some() && !building {
            out.flush()?;
            execute!(
                out,
                ResetColor,
                SetAttribute(Attribute::Reset),
                LeaveAlternateScreen,
                DisableBracketedPaste,
                Show
            )?;
            out.flush()?;
            building = true;
        }
        redraw |= workspace.poll();
        if building {
            out.write_all(std::mem::take(&mut workspace.terminal_output).as_bytes())?;
            out.flush()?;
            if workspace.build.is_none() {
                execute!(out, EnterAlternateScreen, EnableBracketedPaste, Hide)?;
                out.flush()?;
                renderer.invalidate();
                building = false;
                redraw = true;
            }
        }
        if redraw && !building {
            let size = terminal::size()?;
            renderer.draw_workspace(&mut workspace, size, &mut out)?;
        }
        redraw = true;
        if !event::poll(std::time::Duration::from_millis(25))? {
            let pending = workspace.has_pending_input();
            if workspace.timeout() {
                return Ok(());
            }
            redraw = pending || workspace.picker.as_mut().is_some_and(|p| p.poll());
            continue;
        }
        let input = event::read()?;
        if building {
            if let Some(build) = &mut workspace.build {
                match input {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        if key.code == KeyCode::Char('q')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            build.cancel();
                        } else {
                            let _ = build.input(&terminal_key(key));
                        }
                    }
                    Event::Paste(text) => {
                        let _ = build.input(text.as_bytes());
                    }
                    Event::Resize(cols, rows) => build.resize(cols, rows),
                    _ => {}
                }
            }
            continue;
        }
        match input {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.code == KeyCode::Char('l') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    renderer.invalidate();
                    continue;
                }
                if workspace.key(key) {
                    return Ok(());
                }
                if let Some(command) = workspace.editor_mut().config_command.take() {
                    match config.command(&command, &config_dir) {
                        Ok(()) => {
                            workspace.apply_settings(config.settings.clone());
                            workspace.editor_mut().message = "Configuration updated.".into();
                            if out.capacity() != workspace.settings.output_buffer_size {
                                let writer = out.into_inner().map_err(|e| e.into_error())?;
                                out = BufWriter::with_capacity(
                                    workspace.settings.output_buffer_size,
                                    writer,
                                );
                            }
                        }
                        Err(error) => workspace.editor_mut().message = error,
                    }
                }
            }
            Event::Paste(text) => workspace.paste(&text),
            Event::Resize(..) | Event::FocusGained => renderer.invalidate(),
            _ => redraw = false,
        }
    }
}

fn terminal_key(key: crossterm::event::KeyEvent) -> Vec<u8> {
    let text = match key.code {
        KeyCode::Char(ch) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let ch = ch.to_ascii_lowercase();
            if ch == ' ' {
                "\0".into()
            } else if ('@'..='_').contains(&ch) || ch.is_ascii_lowercase() {
                return vec![(ch as u8) & 0x1f];
            } else {
                String::new()
            }
        }
        KeyCode::Char(ch) => ch.to_string(),
        KeyCode::Enter => "\r".into(),
        KeyCode::Backspace => "\x7f".into(),
        KeyCode::Tab => "\t".into(),
        KeyCode::BackTab => "\x1b[Z".into(),
        KeyCode::Esc => "\x1b".into(),
        KeyCode::Null => "\0".into(),
        KeyCode::Up => "\x1b[A".into(),
        KeyCode::Down => "\x1b[B".into(),
        KeyCode::Right => "\x1b[C".into(),
        KeyCode::Left => "\x1b[D".into(),
        KeyCode::Home => "\x1b[H".into(),
        KeyCode::End => "\x1b[F".into(),
        KeyCode::Delete => "\x1b[3~".into(),
        KeyCode::Insert => "\x1b[2~".into(),
        KeyCode::PageUp => "\x1b[5~".into(),
        KeyCode::PageDown => "\x1b[6~".into(),
        _ => String::new(),
    };
    let mut bytes = text.into_bytes();
    if key.modifiers.contains(KeyModifiers::ALT) {
        bytes.insert(0, 0x1b);
    }
    bytes
}

fn run() -> io::Result<()> {
    let mut args = env::args_os().skip(1);
    let first = args.next();
    match first.as_deref().and_then(|s| s.to_str()) {
        Some("--version") => println!("fvim {}", env!("CARGO_PKG_VERSION")),
        Some("--help") | Some("-h") => println!(
            "fvim [file]\ni insert | Esc normal | :w save | :q quit | u undo | Ctrl-R redo\nCtrl-S save | Ctrl-Q quit (twice to discard)\n--version | --config-path | --check-config\n:set | :lua | :source [file] | :colorscheme"
        ),
        Some("--config-path") => println!("{}", install::config_dir()?.join("init.lua").display()),
        Some("--check-config") => {
            config::Config::load(&install::config_dir()?).map_err(io::Error::other)?;
            println!("Configuration OK.");
        }
        Some("--install") => install::install()?,
        Some("--init-config") => install::init_config()?,
        Some("--") => return edit(args.next().map(PathBuf::from)),
        Some(arg) if arg.starts_with('-') => {
            return Err(io::Error::other(format!("Unknown option: {arg}")))
        }
        _ => {
            if args.next().is_some() {
                return Err(io::Error::other("Open one file at a time; use :e to switch files."));
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
