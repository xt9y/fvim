# fvim

A small terminal editor written in Rust. Stage 1 implements basic editing; Vim commands, Lua execution, Neovim features, themes and plugins follow in that order.

```sh
git clone --depth 1 https://github.com/xt9y/fvim.git
cd fvim
make
make install
```

On Unix, `sudo make install` installs the executable in `/usr/local/bin` and creates the invoking user's config. Without sudo it installs in `~/.local/bin`. Windows installs in `%LOCALAPPDATA%\fvim\bin`. The installer preserves existing configuration and persists PATH; open a new terminal if your current PATH does not include the install directory.

Git, GNU make, network access, and a native linker/build toolchain must be available. `make` installs Rust through rustup when Cargo is missing. Windows requires Visual Studio C++ Build Tools; macOS requires Command Line Tools; Linux requires a C linker. These operating-system prerequisites cannot be supplied by a Rust binary or by a missing `make` command.

Configuration is created at `%LOCALAPPDATA%\fvim\init.lua` on Windows and `$XDG_CONFIG_HOME/fvim/init.lua` (default `~/.config/fvim/init.lua`) elsewhere. It is reserved for the Lua configuration stage and is not executed yet.

Run `fvim [file]`. Type to insert, use arrows/Home/End to move, Enter/Backspace/Delete to edit, Ctrl-S to save, and Ctrl-Q to quit (twice to discard changes). Saving an unnamed buffer requires opening a filename first. UTF-8 files and LF/CRLF line endings are supported; tabs and wide/combining text render without emitting file control characters.

`make test` runs behavioral tests. `make clean` removes build output. Installation paths can be staged with `FVIM_PREFIX` and `FVIM_CONFIG_DIR`; `FVIM_NO_PATH=1` disables PATH changes for packaging/tests.

There is exactly one workflow: `TEST`. It builds, tests and verifies repeatable installation on standard GitHub runner images, including x64/ARM64 and preview images. Hosted CI cannot validate every physical terminal; terminal cleanup and rendering are tested independently of a screen.
