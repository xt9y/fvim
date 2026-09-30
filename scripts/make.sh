#!/bin/sh
set -eu
if [ "${1:-build}" = clean ]; then
    rm -rf target
    exit 0
fi
if ! command -v cargo >/dev/null 2>&1; then
    if [ -x "$HOME/.cargo/bin/cargo" ]; then
        PATH="$HOME/.cargo/bin:$PATH"
        export PATH
    else
        command -v cc >/dev/null 2>&1 || {
            echo "A native C linker is required (C build tools / macOS Command Line Tools)." >&2
            exit 1
        }
        installer=$(mktemp)
        trap 'rm -f "$installer"' EXIT HUP INT TERM
        curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o "$installer"
        sh "$installer" -y --profile minimal
        PATH="$HOME/.cargo/bin:$PATH"
        export PATH
    fi
fi
case "${1:-build}" in
    build) cargo build --release ;;
    test) cargo test ;;
    install)
        [ -x target/release/fvim ] || {
            echo "Run make before make install." >&2
            exit 1
        }
        target/release/fvim --install
        ;;
    *) echo "Unknown make target." >&2; exit 1 ;;
esac
