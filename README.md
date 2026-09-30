# fvim

A small terminal editor with a Rust frontend and backend. Stage 2 provides essential Vim editing, search and substitution. Lua execution, full Neovim built-in compatibility, settings, themes and plugins come later.

```sh
git clone --depth 1 https://github.com/xt9y/fvim.git
cd fvim
make
make install
```

On Unix, `sudo make install` installs the executable in `/usr/local/bin` and creates the invoking user's config. Without sudo it installs in `~/.local/bin`. Windows installs in `%LOCALAPPDATA%\fvim\bin`. The installer preserves existing configuration and persists PATH; open a new terminal if your current PATH does not include the install directory.

Git, GNU make, network access, and a native linker/build toolchain must be available. `make` installs Rust through rustup when Cargo is missing. Windows requires Visual Studio C++ Build Tools; macOS requires Command Line Tools; Linux requires a C linker.

Configuration is created at `%LOCALAPPDATA%\fvim\init.lua` on Windows and `$XDG_CONFIG_HOME/fvim/init.lua` (default `~/.config/fvim/init.lua`) elsewhere. It is reserved for the Lua configuration stage and is not executed yet.

Run `fvim [file]`. It starts in Normal mode. Press `i` to insert and Escape to return to Normal mode. `:w` saves; `:q` quits; `:q!` discards changes. Ctrl-S saves and Ctrl-Q quits (twice to discard). UTF-8 files with uniform LF or CRLF line endings and missing final newlines are preserved. Mixed line endings are normalized to the detected format. Tabs, wide/combining text and file control characters are rendered safely.

| Editing | Commands |
| --- | --- |
| Modes | `i a I A o O`, `v V`, Ctrl-V for visual blocks, Escape |
| Motions | `hjkl`, `w W b B e E`, `0 ^ $`, `gg G`, `{ }`, `%`, `f F t T ; ,`, arrows/Home/End/PageUp/PageDown, Ctrl-D/U/F/B |
| Changes | `d c y` with motions or doubled for lines, `x X s S D C Y`, `r`, `J`, `> <`, `p P`; counts such as `2d3w` |
| Text objects | `iw aw iW aW`, quotes, parentheses, brackets and braces; nesting/counts for paired delimiters |
| History/registers | `u`, Ctrl-R redo, `.` repeat; unnamed, named (`"a`), numbered, yank (`"0`) and black-hole (`"_`) registers |
| Search | `/pattern`, `?pattern`, `n N`, `* #`; wrapping searches, counts and operator searches |
| Ex | `:w [file]`, `:q[!]`, `:wq`, `:x`, `:e[!] [file]`, `:enew[!]`, `:undo`, `:redo`, ranged `:d`/`:y`/`:>`/`:<` |
| Substitution | `:s/old/new/`, `:%s/old/new/gc`, numbered/current/last/visual ranges; flags `g c i I n e`, confirmation `y n a q l` |

Patterns use a **subset of Vim's default magic syntax**: `. * ^ $`, character classes, `\d \s \w`, `\< \>` word boundaries, `\( \)` captures, `\|`, `\+ \? \= \{n,m}`, case switches `\c \C`, and magic switches `\v \V \m \M`. Replacements support `&`, `\0`–`\9`, literal `\&`/`\\`, `\t` and `\r` for a newline. Pattern backreferences, lookaround, `\zs`/`\ze`, replacement expressions and other advanced Vim constructs are unsupported. Invalid patterns/flags return errors. This is not full Vim/Neovim compatibility.

Visual selections support deletion, change, yank, paste and indentation; Normal-only commands end the selection before running. Writing a named buffer to another file with `:w file` writes a copy and keeps unsaved changes to the original protected.

`make test` runs behavioral tests. `make clean` removes build output. Installation paths can be staged with `FVIM_PREFIX` and `FVIM_CONFIG_DIR`; `FVIM_NO_PATH=1` disables PATH changes for packaging/tests.

There is exactly one workflow: `TEST`. It builds, tests and verifies repeatable installation, PATH/configuration and real terminal sessions on the standard GitHub runner matrix, including x64/ARM64 and preview images. Windows uses ConPTY; Linux/macOS use PTYs. Hosted CI cannot validate every physical terminal.

Development order: basic frontend/backend; essential Vim editing/search/substitution; measured optimization; Neovim built-in features; settings; themes; plugin support much later. No Neovim process or plugin runtime is required.
