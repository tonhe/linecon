"""Presses real keys in the real window on macOS, and checks the bytes a device receives.

The macOS twin of windows_keys.py: the device is a small TCP server here, reached as a raw
session. Keys go through System Events, so the shell running this needs Accessibility access,
which GitHub's macOS runners grant.

Usage: python3 mac_keys.py path/to/linecon [screenshot.png]
"""
import os
import shutil
import subprocess
import sys
import tempfile
import time

from fake_device import FakeDevice

# Key codes for keys System Events cannot type as text.
RETURN, DELETE, FORWARD_DELETE, ESCAPE, UP = 36, 51, 117, 53, 126


def events(script):
    subprocess.run(["osascript", "-e", f'tell application "System Events" to {script}'], check=True)


def typed(text):
    events(f'keystroke "{text}"')


def code(number, using=None):
    events(f"key code {number}" + (f" using {using}" if using else ""))


def chord(letter, using):
    events(f'keystroke "{letter}" using {using}')


def main():
    source = os.path.abspath(sys.argv[1])
    shot = os.path.abspath(sys.argv[2]) if len(sys.argv) > 2 else None
    home = tempfile.mkdtemp(prefix="linecon-keys-")
    exe = os.path.join(home, "linecon")
    shutil.copy(source, exe)
    open(os.path.join(home, "linecon.portable"), "w").close()

    device = FakeDevice()
    app = subprocess.Popen([exe])
    try:
        time.sleep(8)
        events(f"set frontmost of (first process whose unix id is {app.pid}) to true")
        time.sleep(1)
        chord("k", "command down")
        time.sleep(1)
        typed(f"raw://127.0.0.1:{device.port}")
        time.sleep(0.5)
        code(RETURN)
        if not device.connected.wait(20):
            print("FAIL  the raw session never connected")
            subprocess.run(["screencapture", "-x", shot or "keys-screen.png"])
            return 1
        time.sleep(2)
        device.check("plain typing", lambda: typed("show clock"), b"show clock")
        device.check("Return", lambda: code(RETURN), b"\r")
        device.check("Delete is Backspace", lambda: code(DELETE), b"\x7f")
        device.check("Control+C interrupts the device", lambda: chord("c", "control down"), b"\x03")
        device.check("arrow up", lambda: code(UP), b"\x1b[A")
        device.check("forward delete", lambda: code(FORWARD_DELETE), b"\x1b[3~")
        device.check("Command+C copies and sends nothing", lambda: chord("c", "command down"), b"")
        device.check("Command+K opens the finder, not the device", lambda: chord("k", "command down"), b"")
        device.check("Escape closes the finder", lambda: code(ESCAPE), b"")
        subprocess.run(["pbcopy"], input=b"pasted-line", check=True)
        device.check("Command+V pastes", lambda: chord("v", "command down"), b"pasted-line")
        if shot:
            subprocess.run(["screencapture", "-x", shot])
        chord("w", "command down")
        device.check_closed("Command+W closes the session")
    finally:
        app.kill()
        shutil.rmtree(home, ignore_errors=True)
    return device.report()


if __name__ == "__main__":
    sys.exit(main())
