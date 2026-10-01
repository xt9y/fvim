"""Installed executable and terminal smoke tests; Python is a CI-only dependency."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

VERSION = "fvim " + next(line.split('"')[1] for line in Path("Cargo.toml").read_text().splitlines()
                         if line.startswith("version = "))


def run(*args, env=None, cwd=None):
    return subprocess.check_output(args, env=env, cwd=cwd, text=True, timeout=120).strip()


def install_test(root):
    env = dict(os.environ)
    env.pop("FVIM_NO_PATH", None)
    env.pop("SUDO_USER", None)
    home = root / "home with spaces"
    home.mkdir()
    prefix = root / "prefix with spaces"
    config = root / "configuration"
    env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / ".config"), FVIM_PREFIX=str(prefix), FVIM_CONFIG_DIR=str(config))
    registry = None
    if os.name == "nt":
        import winreg
        registry = winreg.OpenKey(winreg.HKEY_CURRENT_USER, "Environment", 0, winreg.KEY_ALL_ACCESS)
        try:
            old_path, old_type = winreg.QueryValueEx(registry, "Path")
        except FileNotFoundError:
            old_path, old_type = None, winreg.REG_EXPAND_SZ
    try:
        print(run("make", "install", env=env))
        init = config / "init.lua"
        assert init.is_file()
        init.write_text("-- preserved\n", encoding="utf-8")
        run("make", "install", env=env)
        assert init.read_text(encoding="utf-8") == "-- preserved\n"
        binary = prefix / "bin" / ("fvim.exe" if os.name == "nt" else "fvim")
        assert run(str(binary), "--version") == VERSION
        if registry:
            path, _ = winreg.QueryValueEx(registry, "Path")
            parts = [p.rstrip("\\").casefold() for p in path.split(";")]
            assert parts.count(str(binary.parent.resolve()).rstrip("\\").casefold()) == 1, f"Expected {binary.parent}; fvim entries: {[p for p in parts if 'fvim' in p]}"
            fresh = dict(env, PATH=path + ";" + os.environ["PATH"])
            assert run("cmd.exe", "/d", "/c", "fvim --version", env=fresh) == VERSION
        else:
            assert Path(run("sh", "-c", '. "$HOME/.profile"; command -v fvim', env=env)).resolve() == binary.resolve()
            for name in (".profile", ".bashrc", ".zshrc"):
                assert (home / name).read_text().count("# fvim PATH") == 1
            assert (home / ".config/fish/conf.d/fvim-path.fish").is_file()
        print(f"Installed executable: {binary.stat().st_size} bytes")
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



def sudo_install_test(root):
    if os.name == "nt":
        return
    import shutil
    if not shutil.which("sudo"):
        return
    if subprocess.run(["sudo", "-n", "true"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
        print("Sudo install unavailable on this runner.")
        return
    root.chmod(0o755)
    prefix = root / "sudo-prefix"
    config = root / "sudo-config"
    try:
        run("sudo", "-n", "make", "install",
            f"FVIM_PREFIX={prefix}", f"FVIM_CONFIG_DIR={config}", "FVIM_NO_PATH=1")
        init = config / "init.lua"
        assert init.is_file()
        assert init.stat().st_uid == os.getuid(), "Sudo installation must create user-owned config."
        assert run(str(prefix / "bin/fvim"), "--version") == VERSION
        run("sudo", "-n", "chmod", "700", str(prefix / "bin"))
        # Exercise permission escalation only inside this test's staging prefix.
        cleanup = root / "sudo clean workspace"
        (cleanup / "scripts").mkdir(parents=True)
        for name in ("Makefile", "scripts/make.sh"):
            shutil.copyfile(name, cleanup / name)
        run("make", "clean", f"FVIM_PREFIX={prefix}", cwd=cleanup)
        run("sudo", "-n", "test", "!", "-e", str(prefix / "bin/fvim"))
        assert init.is_file() and init.stat().st_uid == os.getuid()
        print("Sudo install, protected-binary cleanup and user config ownership passed.")
    finally:
        if prefix.exists():
            run("sudo", "-n", "chown", "-R", f"{os.getuid()}:{os.getgid()}", str(prefix))


def clean_test(root):
    import shutil
    sandbox = root / "clean workspace"
    (sandbox / "scripts").mkdir(parents=True)
    for name in ("Makefile", "scripts/make.sh", "scripts/make.ps1"):
        shutil.copyfile(name, sandbox / name)
    prefix = sandbox / "prefix with spaces"
    home = sandbox / "home"
    appdata = sandbox / "local appdata"
    binary_name = "fvim.exe" if os.name == "nt" else "fvim"
    binary = prefix / "bin" / binary_name
    binary.parent.mkdir(parents=True)
    binary.write_text("old executable")
    config = prefix / "init.lua"
    config.write_text("-- preserve config\n")
    unrelated = binary.parent / "other-editor"
    unrelated.write_text("keep")
    (sandbox / "target/release").mkdir(parents=True)
    (sandbox / "target/release" / binary_name).write_text("old build")
    env = dict(os.environ, HOME=str(home), USERPROFILE=str(home), LOCALAPPDATA=str(appdata),
               FVIM_PREFIX=str(prefix), FVIM_CONFIG_DIR=str(prefix))
    env.pop("SUDO_USER", None)
    outside = home / ".local/bin" / binary_name
    outside.parent.mkdir(parents=True)
    outside.write_text("outside prefix")
    env["PATH"] = os.pathsep.join([str(outside.parent), os.environ["PATH"]])
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                                     wintypes.LPVOID, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
        kernel.CreateFileW.restype = wintypes.HANDLE
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        # Allow reads/writes but deny deletion while this handle is open.
        handle = kernel.CreateFileW(str(binary), 0x80000000, 3, None, 3, 0x80, None)
        assert handle != wintypes.HANDLE(-1).value, ctypes.get_last_error()
        try:
            result = subprocess.run(["make", "clean"], env=env, cwd=sandbox,
                                    capture_output=True, text=True, timeout=120)
            assert result.returncode != 0, "Locked installed binaries must fail cleanup."
            assert "Cannot remove" in result.stdout + result.stderr
            assert binary.is_file() and config.is_file()
        finally:
            kernel.CloseHandle(handle)
    run("make", "clean", env=env, cwd=sandbox)
    assert not binary.exists(), "make clean must remove the installed executable."
    assert not (sandbox / "target").exists()
    assert config.read_text() == "-- preserve config\n"
    assert unrelated.read_text() == "keep"
    assert outside.read_text() == "outside prefix", "Explicit prefix cleanup must stay confined."
    run("make", "clean", env=env, cwd=sandbox)

    # Never let a developer's real installations become default-clean test data.
    paths = [Path((p.strip('"') if os.name == "nt" else p) or ".") / binary_name
             for p in os.environ.get("PATH", "").split(os.pathsep)]
    if os.name != "nt":
        paths.extend(Path(p) for p in ("/usr/local/bin/fvim", "/opt/homebrew/bin/fvim"))
    if any(p.is_file() or p.is_symlink() for p in paths):
        print("Default clean test skipped: existing installation outside the sandbox.")
        return
    copies = [home / ".local/bin" / binary_name, home / "bin" / binary_name,
              appdata / "fvim/bin" / binary_name] if os.name == "nt" else [
                  home / ".local/bin/fvim", home / "bin/fvim"]
    for n in (1, 2):
        copies.append(sandbox / f"PATH copy {n}" / binary_name)
    for path in copies:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("stale executable")
    reference = sandbox / "reference executable"
    if os.name != "nt":
        reference.write_text("keep symlink target")
        copies[0].unlink()
        copies[0].symlink_to(reference)
    env.pop("FVIM_PREFIX")
    env.pop("FVIM_CONFIG_DIR")
    env["PATH"] = os.pathsep.join([str(p.parent) for p in copies] + [os.environ["PATH"]])
    keep_config = appdata / "fvim/init.lua" if os.name == "nt" else home / ".config/fvim/init.lua"
    keep_config.parent.mkdir(parents=True, exist_ok=True)
    keep_config.write_text("-- user config\n")
    run("make", "clean", env=env, cwd=sandbox)
    assert all(not p.exists() and not p.is_symlink() for p in copies)
    assert keep_config.read_text() == "-- user config\n"
    if reference.exists():
        assert reference.read_text() == "keep symlink target"
    run("make", "clean", env=env, cwd=sandbox)
    print("Clean: installed/PATH duplicates and build output removed; config and other files preserved.")


def expect_file(path, expected):
    deadline = time.monotonic() + 20
    actual = None
    while time.monotonic() < deadline:
        try:
            actual = path.read_bytes()
        except OSError:
            pass
        if actual == expected:
            return
        time.sleep(0.02)
    raise AssertionError(f"Save did not produce {expected!r}; last file contents: {actual!r}")


def exercise_editor(send, expect, clear, path):
    expect(b"NORMAL")
    clear()
    send(b"\x0c")  # Ctrl-L must repaint even an unchanged frame.
    expect(b"NORMAL")
    clear()
    send(b"ihello\rworld\x13")
    expect(b"Saved.")
    expect_file(path, b"hello\nworld")
    clear()
    send(b"\x1b")
    expect(b"NORMAL")
    clear()
    send(b"gg0cwHELLO")
    expect(b"INSERT")
    clear()
    send(b"\x1b")
    expect(b"NORMAL")
    clear()
    send(b":%s/world/EARTH/gc\r")
    expect(b"replace with EARTH")
    clear()
    send(b"y")
    expect(b"1 substitutions")
    clear()
    send(b":w\r")
    expect(b"Saved.")
    expect_file(path, b"HELLO\nEARTH")
    clear()
    send(b"u:w\r")
    expect(b"Saved.")
    expect_file(path, b"HELLO\nworld")
    clear()
    send(b"\x12:w\r")  # Ctrl-R redo.
    expect(b"Saved.")
    expect_file(path, b"HELLO\nEARTH")
    clear()
    send(b"0vld:w\r")
    expect(b"Saved.")
    expect_file(path, b"HELLO\nRTH")
    clear()
    send(b"u:w\r")
    expect(b"Saved.")
    expect_file(path, b"HELLO\nEARTH")
    clear()
    send(b"iX\x11")
    expect(b"Unsaved changes.")
    send(b"\x11")


def windows_terminal_test(binary, root):
    # Native ConPTY; no pip packages or terminal emulator required.
    import ctypes as c
    from ctypes import wintypes as w
    import threading

    k = c.WinDLL("kernel32", use_last_error=True)
    handle = w.HANDLE
    pointer = c.c_void_p
    size_t = c.c_size_t

    class Coord(c.Structure):
        _fields_ = [("X", w.SHORT), ("Y", w.SHORT)]

    class Startup(c.Structure):
        _fields_ = [
            ("cb", w.DWORD), ("lpReserved", w.LPWSTR), ("lpDesktop", w.LPWSTR),
            ("lpTitle", w.LPWSTR), ("dwX", w.DWORD), ("dwY", w.DWORD),
            ("dwXSize", w.DWORD), ("dwYSize", w.DWORD), ("dwXCountChars", w.DWORD),
            ("dwYCountChars", w.DWORD), ("dwFillAttribute", w.DWORD), ("dwFlags", w.DWORD),
            ("wShowWindow", w.WORD), ("cbReserved2", w.WORD), ("lpReserved2", pointer),
            ("hStdInput", handle), ("hStdOutput", handle), ("hStdError", handle),
        ]

    class StartupEx(c.Structure):
        _fields_ = [("StartupInfo", Startup), ("lpAttributeList", pointer)]

    class Process(c.Structure):
        _fields_ = [("hProcess", handle), ("hThread", handle),
                    ("dwProcessId", w.DWORD), ("dwThreadId", w.DWORD)]

    signatures = {
        "CreatePipe": ([c.POINTER(handle), c.POINTER(handle), pointer, w.DWORD], w.BOOL),
        "CreatePseudoConsole": ([Coord, handle, handle, w.DWORD, c.POINTER(handle)], c.c_long),
        "InitializeProcThreadAttributeList": ([pointer, w.DWORD, w.DWORD, c.POINTER(size_t)], w.BOOL),
        "UpdateProcThreadAttribute": ([pointer, w.DWORD, size_t, pointer, size_t, pointer, pointer], w.BOOL),
        "CreateProcessW": ([w.LPCWSTR, w.LPWSTR, pointer, pointer, w.BOOL, w.DWORD,
                            pointer, w.LPCWSTR, pointer, c.POINTER(Process)], w.BOOL),
        "ReadFile": ([handle, pointer, w.DWORD, c.POINTER(w.DWORD), pointer], w.BOOL),
        "WriteFile": ([handle, pointer, w.DWORD, c.POINTER(w.DWORD), pointer], w.BOOL),
        "WaitForSingleObject": ([handle, w.DWORD], w.DWORD),
        "GetExitCodeProcess": ([handle, c.POINTER(w.DWORD)], w.BOOL),
        "TerminateProcess": ([handle, w.UINT], w.BOOL),
        "CloseHandle": ([handle], w.BOOL),
        "ClosePseudoConsole": ([handle], None),
        "DeleteProcThreadAttributeList": ([pointer], None),
    }
    for name, (args, result) in signatures.items():
        fn = getattr(k, name)
        fn.argtypes, fn.restype = args, result

    def check(ok):
        if not ok:
            raise c.WinError(c.get_last_error())

    input_read, input_write, output_read, output_write, console = [handle() for _ in range(5)]
    process = Process()
    attributes = None
    thread = None
    transcript = bytearray()
    condition = threading.Condition()
    path = root / "edited.txt"
    try:
        check(k.CreatePipe(c.byref(input_read), c.byref(input_write), None, 0))
        check(k.CreatePipe(c.byref(output_read), c.byref(output_write), None, 0))
        result = k.CreatePseudoConsole(Coord(100, 24), input_read, output_write, 0, c.byref(console))
        assert result == 0, f"CreatePseudoConsole failed: {result:#x}"
        needed = size_t()
        k.InitializeProcThreadAttributeList(None, 1, 0, c.byref(needed))
        attributes = c.create_string_buffer(needed.value)
        check(k.InitializeProcThreadAttributeList(attributes, 1, 0, c.byref(needed)))
        check(k.UpdateProcThreadAttribute(attributes, 0, 0x00020016, console,
                                         c.sizeof(handle), None, None))
        startup = StartupEx()
        startup.StartupInfo.cb = c.sizeof(startup)
        startup.StartupInfo.dwFlags = 0x00000100  # STARTF_USESTDHANDLES; NULL handles use ConPTY.
        startup.lpAttributeList = c.cast(attributes, pointer)
        command = c.create_unicode_buffer(subprocess.list2cmdline([str(binary), str(path)]))
        check(k.CreateProcessW(str(binary), command, None, None, False, 0x00080000,
                               None, None, c.byref(startup), c.byref(process)))
        k.CloseHandle(input_read)
        input_read = handle()
        k.CloseHandle(output_write)
        output_write = handle()

        def drain():
            data = c.create_string_buffer(65536)
            count = w.DWORD()
            while k.ReadFile(output_read, data, len(data), c.byref(count), None) and count.value:
                with condition:
                    transcript.extend(data.raw[:count.value])
                    condition.notify_all()

        thread = threading.Thread(target=drain, daemon=True)
        thread.start()

        def expect(text):
            deadline = time.monotonic() + 20
            with condition:
                while text not in transcript:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise AssertionError(f"ConPTY did not emit {text!r}: {bytes(transcript)!r}")
                    condition.wait(min(remaining, 0.2))

        def send(data):
            count = w.DWORD()
            check(k.WriteFile(input_write, data, len(data), c.byref(count), None))
            assert count.value == len(data)

        def clear():
            with condition:
                transcript.clear()

        exercise_editor(send, expect, clear, path)
        assert k.WaitForSingleObject(process.hProcess, 10000) == 0, "Editor did not exit."
        exit_code = w.DWORD()
        check(k.GetExitCodeProcess(process.hProcess, c.byref(exit_code)))
        assert exit_code.value == 0
        assert path.read_bytes() == b"HELLO\nEARTH"
        print("ConPTY: Vim modes, operators, visual selection, substitution, undo/redo and saving passed.")
    finally:
        if process.hProcess:
            if k.WaitForSingleObject(process.hProcess, 0) != 0:
                k.TerminateProcess(process.hProcess, 1)
                k.WaitForSingleObject(process.hProcess, 10000)
        if console:
            k.ClosePseudoConsole(console)
        if thread:
            thread.join(timeout=5)
        if attributes:
            k.DeleteProcThreadAttributeList(attributes)
        for value in (input_read, input_write, output_read, output_write,
                      process.hThread, process.hProcess):
            if value:
                k.CloseHandle(value)


def terminal_test(binary, root):
    if os.name == "nt":
        windows_terminal_test(binary, root)
        return
    import fcntl
    import pty
    import select
    import struct
    import termios

    path = root / "edited.txt"
    if sys.platform == "darwin":
        # Apple's script owns PTY creation, avoiding Python preexec callbacks after fork.
        process = subprocess.Popen(
            ["/usr/bin/script", "-q", "/dev/null", "/bin/sh", "-c",
             'stty rows 24 cols 80; exec "$@"', "fvim-test", str(binary), str(path)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            env=dict(os.environ, TERM="xterm-256color"), close_fds=False)
        master = process.stdout.fileno()
        input_fd = process.stdin.fileno()
    else:
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
        def controlling_terminal():
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        process = subprocess.Popen(
            [str(binary), str(path)], stdin=slave, stdout=slave, stderr=slave,
            env=dict(os.environ, TERM="xterm-256color"),
            start_new_session=True, preexec_fn=controlling_terminal)
        os.close(slave)
        input_fd = master
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

    try:
        exercise_editor(lambda data: os.write(input_fd, data), expect, transcript.clear, path)
        assert process.wait(timeout=10) == 0
        assert path.read_bytes() == b"HELLO\nEARTH"
        if sys.platform != "darwin":
            attrs = termios.tcgetattr(master)
            assert attrs[3] & termios.ICANON
            assert attrs[3] & termios.ECHO
        print("PTY: Vim modes, operators, visual selection, substitution, undo/redo and saving passed.")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        if sys.platform == "darwin":
            process.stdin.close()
            process.stdout.close()
        else:
            os.close(master)


assert len(list(Path(".github/workflows").glob("*"))) == 1, "Keep exactly one workflow."
with tempfile.TemporaryDirectory(prefix="fvim-ci-") as directory:
    root = Path(directory)
    executable = install_test(root)
    terminal_test(executable, root)
    sudo_install_test(root)
    clean_test(root)
print("Install, PATH and configuration tests passed.")
