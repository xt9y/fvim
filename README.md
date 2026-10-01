# fvim

A small terminal editor with a Rust frontend and backend. Version 0.6.1 adds native terminal panes and compilation database-aware semantic highlighting alongside Tree-sitter syntax checks, language servers, completion, diagnostics, compiler error navigation, buffers, real splits, project pickers and Lua configuration. Plugin compatibility is still in development.

```sh
git clone --depth 1 https://github.com/xt9y/fvim.git
cd fvim
make
make install
```

On Unix, `sudo make install` installs the executable in `/usr/local/bin` and creates the invoking user's config. Without sudo it installs in `~/.local/bin`. Windows installs in `%LOCALAPPDATA%\fvim\bin`. The installer preserves existing configuration and persists PATH; open a new terminal if your current PATH does not include the install directory.

Git, GNU make, network access, and a native linker/build toolchain must be available. `make` installs Rust through rustup when Cargo is missing. Windows requires Visual Studio C++ Build Tools; macOS requires Command Line Tools; Linux requires a C linker.

Configuration lives in `%LOCALAPPDATA%\fvim` on Windows and `$XDG_CONFIG_HOME/fvim` (default `~/.config/fvim`) elsewhere. `pre_configured.lua` supplies defaults and runs first; `init.lua` runs afterward and overrides them. Installation creates both files and preserves existing contents. The shipped defaults in `config/pre_configured.lua` are also embedded for startup when the installed defaults file is missing. Lua 5.4 is statically linked; no separate Lua installation is needed.

```lua
-- init.lua: override any of the settings in pre_configured.lua
vim.opt.scrolloff = 8
vim.opt.tabstop = 4
vim.opt.shiftwidth = 2
vim.opt.relativenumber = false
vim.opt.background = 'light'
vim.api.nvim_set_hl(0, 'StatusLine', { fg = '#3c3836', bg = '#bdae93' })
```

The UI provides number/sign gutters, scrolling margins, optional wrapping/word breaks, a full-width statusline and a separate command/message area. Tabs, indentation and `o`/`O` consume the configured editing options. `smartindent` currently copies leading indentation and adds one shift after an opening brace; parser-based indentation arrives with syntax tooling. Bracketed paste preserves its supplied whitespace.

`:set option=value`, `:set option` and `:set nooption` change supported options; common abbreviations such as `ts`, `sw`, `nu`, `rnu`, `et`, `so`, `ls`, `ch` and `stl` work. `:lua code` executes Lua, `:source` reloads both config files, and `:source file` / `:luafile file` execute one Lua file. `:colorscheme retrobox` and `:colorscheme default` select shipped UI palettes; define additional palettes in `fvim.themes`. Changing `background` switches the current palette. `vim.api.nvim_set_hl(0, group, values)` supports RGB/indexed foreground/background, bold, italic, underline, reverse and highlight links. `termguicolors=false` selects indexed colors; terminal output honors `NO_COLOR`.

`fvim.opt`, `vim.opt`, `vim.o` and `vim.bo` address the current supported options. This is a small compatibility API: unimplemented options produce errors, and existing Neovim plugin configs cannot run unchanged. Statusline formatting currently supports `%f`, `%F`, `%t`, `%m`, `%M`, `%l`, `%c`, `%v`, `%L`, `%p`, `%P`, `%=` and `%%`. `signcolumn=auto` reserves no space until diagnostics are present.

Native workflow bindings:

| Action | Default binding |
| --- | --- |
| Fuzzy project file picker | Space Space |
| Project grep (Rust regular expressions) | Space Enter |
| Vertical / horizontal split | `hh` / `vv` |
| Save current file and run build | `mm` |
| Open personal init.lua | `cc` |
| All project diagnostics / jump to source | Space d |
| Next / previous diagnostic | `]d` / `[d` |
| Cursor diagnostic / language hover | Space e / `K` |
| Definition / references | `gd` / `gr` |
| Toggle line / block comment | `gcc` / `gbc` (Normal or Visual) |

Inside a picker, arrows or Ctrl-N/P select results, Enter opens the selected file, and the configured split mappings (`hh` / `vv` by default) open it in a split. Ctrl-V/S also open vertical/horizontal splits; Escape closes the picker. Query text uses the same prefix timeout as mappings. `:files`, `:grep [pattern]`, `:split [file]`, `:vsplit [file]`, `:bnext`, `:bprevious`, `:buffer number`, `:buffers`, `:make`, `:config`, `:comment` and `:blockcomment` expose the same workflows. Ctrl-W followed by `h/j/k/l` moves between panes, `w` cycles, `v/s` splits and `q` closes a pane.

Splits share text and undo history, with independent cursors and scroll positions. `:e file` retains the previous buffer, including unsaved edits; `:enew` creates another unnamed buffer. `:e!` reloads the current buffer before switching. `:q` closes the active pane and checks all buffers when closing the last pane; `:q!` permits closing it with unsaved changes. Ctrl-Q checks all buffers and requires a second press to discard. `:wq` saves the current file and follows the same pane/quit rules.

Project scanning starts at the launch directory, runs in a worker thread, skips symlinks, and uses `fvim.workflow.exclude` directory/file names. It currently does not interpret `.gitignore`. Limits are configurable: 20,000 files and 1 MiB per grep file by default; results display at most 2,000 entries. Grep searches saved UTF-8 text, skips binary files, and opens each result at its matching Unicode column. Build commands use an executable/argument list without shell interpretation, run in the launch directory, and run in a real PTY/ConPTY terminal in a horizontal fvim pane, accepting keyboard input and streaming output while the source pane stays visible. Repeated builds reuse the terminal pane. Press Ctrl-\ followed by Ctrl-N to enter Terminal Normal mode and use Ctrl-W pane navigation or Ex commands; `i` resumes terminal input. When the command exits, the editor shows a captured build buffer. Ctrl-C sends a terminal interrupt; Ctrl-Q forcibly stops the build. A failed save prevents the build. If the launch directory contains `Makefile`, `makefile` or `GNUmakefile`, `mm` and `:make` use `fvim.workflow.make`; otherwise they use `fvim.workflow.fallback`, defaulting to `{ 'c', 'build', 'run' }`. Arguments after `:make` are forwarded to either command, preserving quoted arguments (`:make --flag "two words"`). Compiler errors join syntax and LSP diagnostics in the Space d screen; Enter jumps to the selected location.

```lua
-- Personal workflow overrides in init.lua
vim.keymap.del('n', 'hh')
vim.keymap.set('n', 'zz', 'vsplit') -- also applies inside pickers
vim.keymap.set('n', '<leader>f', ':files<CR>')
fvim.workflow.timeout_ms = 300
fvim.workflow.make = { 'make' }
fvim.workflow.fallback = { 'c', 'build', 'run' }
vim.filetype.add { extension = { shader = 'hlsl' } }
fvim.filetype_options.hlsl = { tabstop=4, shiftwidth=2, expandtab=true }
fvim.comments.hlsl = { line='//', open='/*', close='*/' }
```

`vim.keymap.set`/`del` support Normal (`n`) and Visual (`v`) mode mappings to the listed native actions plus buffer next/previous. Lua callbacks and arbitrary Ex mappings are not supported yet. Prefix mappings override the corresponding Vim sequence and wait up to `timeout_ms`; counts, operators, searches, Insert mode and unmatched prefixes continue through the Vim editing engine. `.ll`/`.llvm`, `.zig`, `.odin` and `.hlsl`/`.hlsli` have shipped filetype overrides and comment syntax. LLVM and Zig provide line comments only. Filetype indentation overrides the global indentation values; change `fvim.filetype_options` to customize it.

Tree-sitter grammars are embedded for C, C++, Lua, Zig, Odin, Bash, JSON, Markdown and HLSL; LLVM uses native token highlighting. Syntax errors and compiler/LSP diagnostics use severity colors, signs, underlines, four spaces followed by `●` virtual text, and rounded cursor popups with the source. Diagnostic display waits until leaving Insert mode by default. Space d opens all project diagnostics; normal Vim `dd` remains line deletion.

The native LSP client starts `clangd --background-index`, `zls`, `ols` and `slangd` for their configured filetypes. Install the server executables on PATH; an existing Neovim Mason bin directory is also searched. Servers are external tools and are not downloaded automatically. Missing or failed servers appear in the diagnostics screen footer. Completion triggers after the configured `updatetime` (300 ms); Ctrl-Space requests it explicitly. Tab/Shift-Tab or Ctrl-N/P select, Enter accepts a selected item, and Ctrl-Y accepts the first item. LSP snippet completions expand parameter/control-flow placeholders; the active placeholder is selected, Tab/Shift-Tab moves forward/backward, and `$0` exits snippet navigation at the final cursor position. Completion details are shown beside labels when the server provides them. Completion resolve and workspace edits are not implemented. Hover, definitions and references use the active document's language server.

Clangd supplies semantic colors for types, functions, variables, fields and constants using the project's compilation database. fvim searches source ancestors, then up to three levels under `build`, `.build`, `out` and `target` for `compile_commands.json`, and passes the discovered directory to clangd. Database creation and changes refresh the project's clangd sessions while interactive builds keep running; completed builds also refresh them. For another location, set `vim.lsp.config('clangd', { compile_commands_dir='path/to/build' })`, or pass clangd's `--compile-commands-dir` argument explicitly. Syntax colors remain available when upgrading with an older preserved palette; personal highlight overrides take precedence.

```lua
vim.diagnostic.config { virtual_text=false, underline=true, update_in_insert=false }
vim.lsp.config('clangd', { cmd={'clangd', '--background-index'}, root_markers={'compile_commands.json', 'compile_flags.txt', '.git'} })
vim.lsp.enable { 'clangd', 'zls', 'ols', 'slangd' }
vim.opt.autocomplete = true
vim.opt.pumheight = 5
vim.opt.updatetime = 300
```

`fvim --check-config` validates configuration without opening a terminal; `fvim --config-path` prints the personal config path. Errors identify the Lua file/option. Failed runtime updates retain the previous effective options/highlights; Lua filesystem/process side effects are not rolled back. Existing defaults are preserved on upgrades: new shipped defaults can be reviewed in `config/pre_configured.lua` before replacing an edited installed copy.

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

Editing changes only affected lines; grouped undo stores the original changed span. Cursor offsets use indexed line lengths, dirty checks compare exact saved content incrementally, and motions visit text without copying the file. Rendering buffers output and redraws changed rows instead of clearing the screen on every key. Ctrl-L, focus return and resize force a full repaint to recover a damaged screen. Searches avoid collecting all match positions and repeatedly decoding file prefixes. Inserting/deleting rows still rebuilds the line index; wide undo spans and searches may still touch the whole file.

The `TEST` workflow enforces allocation budgets and prints release-mode timing/allocation probes on every runner. Timing reports are informational because hosted CPU speed varies; deterministic allocation and rendering checks catch regressions. To reproduce the probes locally: `cargo test --release --locked -- --ignored --nocapture`.

`make test` runs behavioral tests. `make clean` removes build output and installed executables named `fvim` from the standard install locations and every current PATH directory, preserving configuration and other files. Unix cleanup requests sudo when an old system copy is protected; Windows cleanup fails clearly if an executable is locked or needs elevated permissions. With `FVIM_PREFIX` set, installed-binary cleanup is confined to that prefix. Installation paths can be staged with `FVIM_PREFIX` and `FVIM_CONFIG_DIR`; `FVIM_NO_PATH=1` disables PATH changes for packaging/tests.

There is exactly one workflow: `TEST`. It builds, tests and verifies repeatable installation, PATH/configuration and real terminal sessions on the standard GitHub runner matrix, including x64/ARM64 and preview images. Windows uses ConPTY; Linux/macOS use PTYs. Hosted CI cannot validate every physical terminal.

Development order: basic frontend/backend; essential Vim editing/search/substitution; measured optimization; Neovim built-in features; settings; themes; plugin support much later. No Neovim process or plugin runtime is required.
