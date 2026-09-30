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
        assert run(str(binary), "--version") == "fvim 0.1.0"
        if registry:
            path, _ = winreg.QueryValueEx(registry, "Path")
            parts = [p.rstrip("\\").casefold() for p in path.split(";")]
            assert parts.count(str(binary.parent.resolve()).rstrip("\\").casefold()) == 1, f"Expected {binary.parent}; fvim entries: {[p for p in parts if 'fvim' in p]}"
            fresh = dict(env, PATH=path + ";" + os.environ["PATH"])
            assert run("fvim", "--version", env=fresh) == "fvim 0.1.0"
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
        assert run(str(prefix / "bin/fvim"), "--version") == "fvim 0.1.0"
        print("Sudo install and invoking-user configuration ownership passed.")
    finally:
        if prefix.exists():
            run("sudo", "-n", "chown", "-R", f"{os.getuid()}:{os.getgid()}", str(prefix))


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

        expect(b"Ctrl-S save")
        with condition:
            transcript.clear()
        send(b"hello\rworld\x13")
        expect(b"Saved.")
        assert path.read_bytes() == b"hello\nworld"
        with condition:
            transcript.clear()
        send(b"X\x11")
        expect(b"Unsaved changes.")
        send(b"\x11")
        assert k.WaitForSingleObject(process.hProcess, 10000) == 0, "Editor did not exit."
        exit_code = w.DWORD()
        check(k.GetExitCodeProcess(process.hProcess, c.byref(exit_code)))
        assert exit_code.value == 0
        assert path.read_bytes() == b"hello\nworld"
        print("ConPTY: editing, saving and discard confirmation passed.")
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
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
    def controlling_terminal():
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    process = subprocess.Popen([str(binary), str(path)], stdin=slave, stdout=slave, stderr=slave,
                               env=dict(os.environ, TERM="xterm-256color"),
                               start_new_session=True, preexec_fn=controlling_terminal)
    os.close(slave)
    pid = process.pid
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
    sudo_install_test(root)
print("Install, PATH and configuration tests passed.")
