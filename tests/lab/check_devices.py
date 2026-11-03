#!/usr/bin/env python3
"""Logs in to real lab devices through the linecon CLI on a real terminal.

usage: check_devices.py <linecon binary> <name> <host> <user> <password> <command> [<marker>] [-- extra args]

Exits non-zero unless the login works and the command's output shows the marker.
Default settings are the point: pass no extra args to prove a device needs no flags.
"""
import os
import pty
import re
import select
import signal
import sys
import tempfile
import time

TIMEOUT = 60


def run(binary, name, host, user, password, command, marker, extra):
    home = tempfile.mkdtemp(prefix="linecon-lab-")
    pid, fd = pty.fork()
    if pid == 0:
        os.environ.update({"HOME": home, "XDG_CONFIG_HOME": home, "TERM": "xterm-256color"})
        os.execv(binary, [binary, "ssh", host, user] + extra)
    seen = ""
    stage = "login"
    deadline = time.time() + TIMEOUT
    sent_command_at = None
    result = None
    while time.time() < deadline:
        ready, _, _ = select.select([fd], [], [], 0.5)
        if ready:
            try:
                data = os.read(fd, 65536)
            except OSError:
                break
            if not data:
                break
            seen += data.decode("utf-8", "replace")
        if "Trust this key?" in seen and stage == "login":
            os.write(fd, b"o\r")
            seen = seen.replace("Trust this key?", "TRUSTED")
        if re.search(r"[Pp]assword:\s*$", seen) and stage == "login":
            os.write(fd, password.encode() + b"\r")
            seen = re.sub(r"[Pp]assword:\s*$", "SENT-PASSWORD\n", seen)
            stage = "prompt"
        if stage == "prompt" and re.search(r"[>#%$]\s*$", seen):
            os.write(fd, command.encode() + b"\r")
            stage = "command"
            sent_command_at = time.time()
        if stage == "command" and marker in seen[seen.rfind(command) + len(command):]:
            result = True
            break
        # Pagers would otherwise hold the marker back.
        if "--More--" in seen[-40:] or "More" in seen[-12:]:
            os.write(fd, b" ")
            seen += "\n"
        if sent_command_at and time.time() - sent_command_at > 30:
            break
    try:
        os.write(fd, b"\r~.")
    except OSError:
        pass
    time.sleep(0.3)
    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    os.waitpid(pid, 0)
    print(f"{'PASS' if result else 'FAIL'}  {name:10} {' '.join(extra) or '(no flags)'}")
    if not result:
        print(seen[-1500:])
    return bool(result)


def main():
    args = sys.argv[1:]
    extra = []
    if "--" in args:
        at = args.index("--")
        args, extra = args[:at], args[at + 1:]
    binary, name, host, user, password, command = args[:6]
    marker = args[6] if len(args) > 6 else "ersion"
    sys.exit(0 if run(binary, name, host, user, password, command, marker, extra) else 1)


if __name__ == "__main__":
    main()
