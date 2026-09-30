"""Installed executable and terminal smoke tests; Python is a CI-only dependency."""
import os
from pathlib import Path
import subprocess
import tempfile
import time


def run(*args, env=None):
    return subprocess.check_output(args, env=env, text=True).strip()


def install_test(root):
    env = dict(os.environ)
    env.pop("FVIM_NO_PATH", None)
    env.pop("SUDO_USER", None)
    home = root / "home with spaces"
    home.mkdir()
    prefix = root / "prefix with spaces"
    config = root / "configuration"
    env.update(HOME=str(home), FVIM_PREFIX=str(prefix), FVIM_CONFIG_DIR=str(config))
    registry = None
    if os.name == "nt":
        import winreg
        registry = winreg.OpenKey(winreg.HKEY_CURRENT_USER, "Environment", 0, winreg.KEY_ALL_ACCESS)
        try:
            old_path, old_type = winreg.QueryValueEx(registry, "Path")
        except FileNotFoundError:
            old_path, old_type = None, winreg.REG_EXPAND_SZ
    try:
        run("make", "install", env=env)
        init = config / "init.lua"
        assert init.is_file()
        init.write_text("-- preserved\n", encoding="utf-8")
        run("make", "install", env=env)
        assert init.read_text(encoding="utf-8") == "-- preserved\n"
        binary = prefix / "bin" / ("fvim.exe" if os.name == "nt" else "fvim")
        assert run(str(binary), "--version") == "fvim 0.1.0"
        if registry:
            path, _ = winreg.QueryValueEx(registry, "Path")
            parts = [p.rstrip("\\").casefold() for p in path.split(";")]
            assert parts.count(str(binary.parent).rstrip("\\").casefold()) == 1
            fresh = dict(env, PATH=path + ";" + os.environ["PATH"])
            assert run("fvim", "--version", env=fresh) == "fvim 0.1.0"
        else:
            assert run("sh", "-c", '. "$HOME/.profile"; command -v fvim', env=env) == str(binary)
            for name in (".profile", ".bashrc", ".zshrc"):
                assert (home / name).read_text().count("# fvim PATH") == 1
            assert (home / ".config/fish/conf.d/fvim-path.fish").is_file()
        return binary
    finally:
        if registry:
            if old_path is None:
                try:
                    winreg.DeleteValue(registry, "Path")
                except FileNotFoundError:
                    pass
            else:
                winreg.SetValueEx(registry, "Path", 0, old_type, old_path)
            registry.Close()


def terminal_test(binary, root):
    if os.name == "nt":
        print("Windows: native build/install and rendering tests; interactive ConPTY coverage pending.")
        return
    import fcntl
    import pty
    import select
    import struct
    import termios

    path = root / "edited.txt"
    pid, master = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.execv(str(binary), [str(binary), str(path)])
    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
    transcript = bytearray()

    def expect(text):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if text in transcript:
                return
            if select.select([master], [], [], 0.2)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    break
                if not data:
                    break
                transcript.extend(data)
        raise AssertionError(f"Terminal did not emit {text!r}: {bytes(transcript)!r}")

    reaped = False
    try:
        expect(b"Ctrl-S save")
        transcript.clear()
        os.write(master, b"hello\rworld\x13")
        expect(b"Saved.")
        assert path.read_bytes() == b"hello\nworld"
        transcript.clear()
        os.write(master, b"X\x11")
        expect(b"Unsaved changes.")
        os.write(master, b"\x11")
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            ended, status = os.waitpid(pid, os.WNOHANG)
            if ended:
                reaped = True
                assert os.waitstatus_to_exitcode(status) == 0
                assert path.read_bytes() == b"hello\nworld"
                attrs = termios.tcgetattr(master)
                assert attrs[3] & termios.ICANON
                assert attrs[3] & termios.ECHO
                print("PTY: editing, saving, discard confirmation and terminal restoration passed.")
                return
            time.sleep(0.05)
        raise AssertionError("Editor did not exit.")
    finally:
        if not reaped:
            try:
                os.kill(pid, 9)
                os.waitpid(pid, 0)
            except ProcessLookupError:
                pass
        os.close(master)


assert len(list(Path(".github/workflows").glob("*"))) == 1, "Keep exactly one workflow."
with tempfile.TemporaryDirectory(prefix="fvim-ci-") as directory:
    root = Path(directory)
    executable = install_test(root)
    terminal_test(executable, root)
print("Install, PATH and configuration tests passed.")
