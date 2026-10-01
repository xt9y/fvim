mod buffer;
mod command;
mod config;
mod editor;
mod install;
mod motion;
#[cfg(test)]
mod performance;
mod renderer;

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
use editor::Editor;
use renderer::Renderer;
use std::env;
use std::io::{self, BufWriter, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;

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
    let mut editor = Editor::new(Buffer::open(path)?);
    editor.settings = config.settings.clone();
    let _terminal = Terminal::enter()?;
    let mut renderer = Renderer::default();
    let mut out = BufWriter::with_capacity(editor.settings.output_buffer_size, io::stdout().lock());
    let mut redraw = true;
    loop {
        if redraw {
            let size = terminal::size()?;
            editor.page_rows = usize::from(size.1)
                .saturating_sub(editor.settings.cmdheight)
                .saturating_sub(usize::from(editor.settings.laststatus >= 2));
            renderer.draw(&editor, size, &mut out)?;
        }
        redraw = true;
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.code == KeyCode::Char('l') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    renderer.invalidate();
                    continue;
                }
                if editor.key(key) {
                    return Ok(());
                }
                if let Some(command) = editor.config_command.take() {
                    match config.command(&command, &config_dir) {
                        Ok(()) => {
                            editor.settings = config.settings.clone();
                            editor.message = "Configuration updated.".into();
                            if out.capacity() != editor.settings.output_buffer_size {
                                let writer = out.into_inner().map_err(|e| e.into_error())?;
                                out = BufWriter::with_capacity(
                                    editor.settings.output_buffer_size,
                                    writer,
                                );
                            }
                        }
                        Err(error) => editor.message = error,
                    }
                }
            }
            Event::Paste(text) => editor.paste(&text),
            Event::Resize(..) | Event::FocusGained => renderer.invalidate(),
            _ => redraw = false,
        }
    }
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
