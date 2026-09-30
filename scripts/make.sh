#!/bin/sh
set -eu
if [ "${1:-build}" = clean ]; then
    if [ -e target ] || [ -L target ]; then rm -r -- target; fi
    failed=0
    remove_binary() {
        # Do not mistake an inaccessible candidate for a missing file.
        # Plain directories named fvim are not installed executables.
        if [ -d "$1" ] && [ ! -L "$1" ]; then return; fi
        present=0
        if [ -f "$1" ] || [ -L "$1" ]; then present=1; fi
        if rm -f -- "$1"; then
            if [ "$present" = 1 ]; then
                printf 'Removed %s\n' "$1"
            fi
        elif command -v sudo >/dev/null 2>&1; then
            printf 'Removing protected fvim with sudo: %s\n' "$1"
            if sudo -- rm -f -- "$1"; then
                printf 'Removed %s\n' "$1"
            else
                failed=1
            fi
        else
            printf 'Cannot remove %s; rerun make clean with sufficient permissions.\n' "$1" >&2
            failed=1
        fi
    }
    if [ "${FVIM_PREFIX+x}" = x ]; then
        # An explicit packaging prefix confines cleanup to that installation.
        remove_binary "${FVIM_PREFIX:-.}/bin/fvim"
    else
        clean_home=${HOME:-}
        if [ -n "${SUDO_USER:-}" ] && [ "$(id -u)" = 0 ]; then
            clean_home=$(sudo -H -u "$SUDO_USER" -- sh -c 'printf "%s" "$HOME"')
        fi
        if [ -n "$clean_home" ]; then
            remove_binary "$clean_home/.local/bin/fvim"
            remove_binary "$clean_home/bin/fvim"
        fi
        remove_binary /usr/local/bin/fvim
        remove_binary /opt/homebrew/bin/fvim
        remaining=${PATH:-}
        while :; do
            directory=${remaining%%:*}
            remove_binary "${directory:-.}/fvim"
            case "$remaining" in
                *:*) remaining=${remaining#*:} ;;
                *) break ;;
            esac
        done
    fi
    exit "$failed"
fi
if [ "${1:-build}" = install ]; then
    [ -x target/release/fvim ] || {
        echo "Run make before make install." >&2
        exit 1
    }
    exec target/release/fvim --install
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
    build) cargo build --release --locked ;;
    test) cargo test --locked ;;
    *) echo "Unknown make target." >&2; exit 1 ;;
esac
