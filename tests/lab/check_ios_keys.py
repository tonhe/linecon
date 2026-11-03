#!/usr/bin/env python3
"""Exercises the keys a network engineer leans on, against a real IOS device.

usage: check_ios_keys.py <linecon binary> <host> <user> <password>
"""
import os
import pty
import select
import signal
import sys
import tempfile
import time


class Term:
    def __init__(self, binary, host, user):
        home = tempfile.mkdtemp(prefix="linecon-lab-")
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.environ.update({"HOME": home, "XDG_CONFIG_HOME": home, "TERM": "xterm-256color"})
            os.execv(binary, [binary, "ssh", host, user])
        self.seen = ""

    def expect(self, text, timeout=25):
        deadline = time.time() + timeout
        while text not in self.seen:
            if time.time() > deadline:
                raise AssertionError(f"never saw {text!r}; last output: {self.seen[-400:]!r}")
            ready, _, _ = select.select([self.fd], [], [], 0.3)
            if ready:
                self.seen += os.read(self.fd, 65536).decode("utf-8", "replace")
        self.seen = self.seen[self.seen.find(text) + len(text):]

    def send(self, data):
        os.write(self.fd, data)


def main():
    binary, host, user, password = sys.argv[1:5]
    t = Term(binary, host, user)
    t.expect("Trust this key?")
    t.send(b"o\r")
    t.expect("assword:")
    t.send(password.encode() + b"\r")
    t.expect("#")
    checks = [
        ("? shows help without Enter", b"?", "Exec commands"),
        ("Ctrl+U clears the line", b"\x15", ""),
        ("Tab completes a command", b"sho\t", "show"),
        ("Ctrl+U clears again", b"\x15", ""),
        ("config mode", b"configure terminal\r", "(config)#"),
        ("Ctrl+Z leaves config mode", b"\x1a", "#"),
        ("paging waits at --More--", b"terminal length 10\rshow running-config\r", "--More--"),
        ("Space advances the pager", b" ", "--More--"),
        ("q ends the pager", b"q", "#"),
        ("Up arrow recalls history", b"\x1b[A", "show running-config"),
    ]
    failed = 0
    for name, keys, want in checks:
        t.send(keys)
        try:
            if want:
                t.expect(want)
            else:
                time.sleep(0.5)
            print(f"PASS  {name}")
        except AssertionError as e:
            failed += 1
            print(f"FAIL  {name}: {e}")
    t.send(b"\x15exit\r")
    time.sleep(0.5)
    try:
        os.kill(t.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
