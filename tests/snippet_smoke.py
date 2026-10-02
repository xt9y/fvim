"""Installed PTY smoke test for LSP snippets and the diagnostics mapping."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = Path(__file__).with_name("snippet_fixture.py").resolve()


def wait_file(path, expected):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            if path.read_bytes() == expected:
                return
        except OSError:
            pass
        time.sleep(0.02)
    raise AssertionError(f"Expected {expected!r}; got {path.read_bytes() if path.exists() else None!r}")


if os.name == "nt":
    sys.exit(0)

with tempfile.TemporaryDirectory(prefix="fvim-snippet-") as directory:
    root = Path(directory)
    home = root / "home"
    home.mkdir()
    prefix = root / "prefix"
    config = root / "config"
    env = dict(
        os.environ,
        HOME=str(home),
        FVIM_PREFIX=str(prefix),
        FVIM_CONFIG_DIR=str(config),
        FVIM_NO_PATH="1",
        TERM="xterm-256color",
    )
    subprocess.check_call(["make", "install"], cwd=ROOT, env=env)
    binary = prefix / "bin" / "fvim"
    init = config / "init.lua"
    python = sys.executable.replace("\\", "/")
    fixture = str(FIXTURE).replace("\\", "/")
    with init.open("a", encoding="utf-8") as stream:
        stream.write("\nvim.opt.updatetime=500\n")
        stream.write("vim.lsp.config('clangd',{cmd={" + json.dumps(python) + "," + json.dumps(fixture) + "}})\n")
        stream.write("vim.lsp.enable({'zls','ols','slangd'}, false)\n")

    source = root / "snippet.c"
    source.write_bytes(b"")
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))

    def controlling_terminal():
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    process = subprocess.Popen(
        [str(binary), str(source)],
        stdin=slave,
        stdout=slave,
        stderr=slave,
        cwd=root,
        env=env,
        start_new_session=True,
        preexec_fn=controlling_terminal,
    )
    os.close(slave)
    transcript = bytearray()

    def send(data):
        os.write(master, data)

    def expect(text):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if text in transcript:
                return
            if select.select([master], [], [], 0.2)[0]:
                data = os.read(master, 65536)
                if not data:
                    break
                transcript.extend(data)
        raise AssertionError(f"Terminal did not emit {text!r}: {bytes(transcript)!r}")

    def clear():
        transcript.clear()

    try:
        expect(b"snippet.c")
        clear()
        completion_started = time.monotonic()
        send(b"iX")
        expect(b"for-loop")
        expect(b"for (init; condition; inc) { statements }")
        assert time.monotonic() - completion_started < 0.25, "completion waited for updatetime"
        clear()
        send(b"\t\r")
        expect(b"init-statement")
        clear()
        # Fill the first two placeholders, jump backward with Shift-Tab, replace
        # the first one, then continue forward through the remaining stops.
        send(b"i=0\ti<3\x1b[Zj=0\t\ti++\twork()\t;\x13")
        expect(b"Saved.")
        wait_file(source, b"for (j=0; i<3; i++) { work() };")

        clear()
        send(b"\x1b")
        expect(b"\x1b[2 q")
        clear()
        send(b" d")
        expect(b"Diagnostics >")
        clear()
        send(b"\x1b")
        expect(b"snippet.c")
        clear()
        send(b"dd:w\r")
        expect(b"Saved.")
        wait_file(source, b"")
        send(b"\x11")
        assert process.wait(timeout=10) == 0
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        os.close(master)

print("Snippet completion, placeholder navigation and Space+d diagnostics passed.")
