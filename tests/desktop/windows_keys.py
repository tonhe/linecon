"""Presses real keys in the real window on Windows, and checks the bytes a device receives.

The device is a small TCP server here, reached as a raw session, so every byte the app sends
can be compared exactly. Controls are found by their screen reader names, which checks those
names on Windows as a side effect.

Usage: python windows_keys.py path\\to\\linecon.exe [screenshot.png]
Needs: pip install pywinauto
"""
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time

from pywinauto import Application, keyboard

received = bytearray()
lock = threading.Lock()
connected = threading.Event()
closed = threading.Event()


def serve(listener):
    conn, _ = listener.accept()
    connected.set()
    conn.sendall(b"test device ready\r\nrouter> ")
    while True:
        data = conn.recv(4096)
        if not data:
            closed.set()
            return
        with lock:
            received.extend(data)


def take():
    with lock:
        data = bytes(received)
        received.clear()
    return data


def expect(name, keys, wanted, settle=1.5):
    """Presses `keys` and checks the device got exactly `wanted`, nothing more."""
    take()
    keyboard.send_keys(keys, pause=0.05, with_spaces=True)
    deadline = time.time() + 8
    while time.time() < deadline:
        time.sleep(0.1)
        with lock:
            if len(received) >= max(len(wanted), 1):
                break
    time.sleep(settle)
    got = take()
    ok = got == wanted
    print(f"{'PASS' if ok else 'FAIL'}  {name}: sent {got!r}" + ("" if ok else f", wanted {wanted!r}"))
    return ok


def set_clipboard(text):
    subprocess.run(["powershell", "-NoProfile", "-Command", f"Set-Clipboard -Value '{text}'"], check=True)


def screenshot(path):
    subprocess.run(
        [
            "powershell", "-NoProfile", "-Command",
            "Add-Type -AssemblyName System.Windows.Forms,System.Drawing; "
            "$b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds; "
            "$i=New-Object System.Drawing.Bitmap $b.Width,$b.Height; "
            "[System.Drawing.Graphics]::FromImage($i).CopyFromScreen($b.Location,[System.Drawing.Point]::Empty,$b.Size); "
            f"$i.Save('{path}')",
        ],
        check=False,
    )


def main():
    source = os.path.abspath(sys.argv[1])
    shot = os.path.abspath(sys.argv[2]) if len(sys.argv) > 2 else None
    # A portable copy, so the run starts with no settings and leaves none behind.
    home = tempfile.mkdtemp(prefix="linecon-keys-")
    exe = os.path.join(home, "linecon.exe")
    shutil.copy(source, exe)
    open(os.path.join(home, "linecon.portable"), "w").close()

    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    port = listener.getsockname()[1]
    threading.Thread(target=serve, args=(listener,), daemon=True).start()

    app = Application(backend="uia").start(exe, timeout=30)
    results = []
    try:
        window = app.window(title="linecon0")
        window.wait("visible ready", timeout=60)
        window.set_focus()
        quick = window.child_window(title="Quick connect", control_type="Edit")
        quick.wait("visible", timeout=30)
        results.append(("the quick connect box has its name", True))
        print("PASS  the quick connect box has its name")
        quick.click_input()
        keyboard.send_keys(f"raw://127.0.0.1:{port}" + "{ENTER}", pause=0.03)
        if not connected.wait(20):
            print("FAIL  the raw session never connected")
            return 1
        time.sleep(2)
        checks = [
            ("plain typing", "show{SPACE}clock", b"show clock"),
            ("Enter", "{ENTER}", b"\r"),
            ("Backspace", "{BACKSPACE}", b"\x7f"),
            ("Ctrl+C interrupts the device", "^c", b"\x03"),
            ("Ctrl+V goes to the device", "^v", b"\x16"),
            ("Alt+x arrives once, with Escape", "%x", b"\x1bx"),
            ("arrow up", "{UP}", b"\x1b[A"),
            ("Delete", "{DEL}", b"\x1b[3~"),
            ("Shift+Delete is Delete, not cut", "+{DEL}", b"\x1b[3;2~"),
            ("Ctrl+Insert copies and sends nothing", "^{INSERT}", b""),
            ("Ctrl+Shift+K opens the finder, not the device", "^+k", b""),
            ("Escape closes the finder", "{ESC}", b""),
        ]
        for name, keys, wanted in checks:
            results.append((name, expect(name, keys, wanted)))
        set_clipboard("pasted-line")
        results.append(("Shift+Insert pastes", expect("Shift+Insert pastes", "+{INSERT}", b"pasted-line")))
        results.append(("Ctrl+Shift+V pastes", expect("Ctrl+Shift+V pastes", "^+v", b"pasted-line")))
        if shot:
            screenshot(shot)
        keyboard.send_keys("^+w")
        ok = closed.wait(10)
        print(f"{'PASS' if ok else 'FAIL'}  Ctrl+Shift+W closes the session")
        results.append(("close", ok))
    finally:
        app.kill()
        shutil.rmtree(home, ignore_errors=True)
    failed = [n for n, ok in results if not ok]
    print(f"{len(results) - len(failed)} passed, {len(failed)} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
