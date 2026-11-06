"""Presses real keys in the real window on Windows, and checks the bytes a device receives.

The device is a small TCP server here, reached as a raw session, so every byte the app sends
can be compared exactly. Controls are found by their screen reader names, which checks those
names on Windows as a side effect.

Usage: python windows_keys.py path\\to\\linecon.exe [screenshot.png]
Needs: pip install pywinauto
"""
import os
import shutil
import subprocess
import sys
import tempfile
import time

from pywinauto import Application, keyboard

from fake_device import FakeDevice


def keys(sequence):
    return lambda: keyboard.send_keys(sequence, pause=0.05, with_spaces=True)


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

    device = FakeDevice()
    app = Application(backend="uia").start(exe, timeout=30, wait_for_idle=False)
    try:
        window = app.window(title="linecon0")
        window.wait("visible ready", timeout=60)
        window.set_focus()
        quick = window.child_window(title="Quick connect", control_type="Edit")
        quick.wait("visible", timeout=30)
        device.record("the quick connect box has its name", True)
        quick.click_input()
        keyboard.send_keys(f"raw://127.0.0.1:{device.port}" + "{ENTER}", pause=0.03)
        if not device.connected.wait(20):
            device.record("the raw session connects", False)
            if shot:
                screenshot(shot)
            return device.report()
        time.sleep(2)
        device.check("plain typing", keys("show clock"), b"show clock")
        device.check("Enter", keys("{ENTER}"), b"\r")
        device.check("Backspace", keys("{BACKSPACE}"), b"\x7f")
        device.check("Ctrl+C interrupts the device", keys("^c"), b"\x03")
        device.check("Alt+x arrives once, with Escape", keys("%x"), b"\x1bx")
        device.check("arrow up", keys("{UP}"), b"\x1b[A")
        device.check("Delete", keys("{DEL}"), b"\x1b[3~")
        device.check("Shift+Delete is Delete, not cut", keys("+{DEL}"), b"\x1b[3;2~")
        device.check("Ctrl+Insert copies and sends nothing", keys("^{INSERT}"), b"")
        device.check("Ctrl+Shift+K opens the finder, not the device", keys("^+k"), b"")
        device.check("Escape closes the finder", keys("{ESC}"), b"")
        set_clipboard("pasted-line")
        # With text on the clipboard, which is when it could be mistaken for a paste.
        device.check("Ctrl+V goes to the device", keys("^v"), b"\x16")
        device.check("Shift+Insert pastes", keys("+{INSERT}"), b"pasted-line")
        device.check("Ctrl+Shift+V pastes", keys("^+v"), b"pasted-line")
        if shot:
            screenshot(shot)
        keys("^+w")()
        device.check_closed("Ctrl+Shift+W closes the session")
    finally:
        app.kill()
        shutil.rmtree(home, ignore_errors=True)
    return device.report()


if __name__ == "__main__":
    sys.exit(main())
