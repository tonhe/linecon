# Decisions

Choices made where the requirements left room, newest last.

## 2026-11-03: Commit identity uses the GitHub noreply address

The repo is public and history is hard to rewrite, so commits use the account's
noreply address instead of a personal email.

## 2026-11-03: Rust, alacritty_terminal and egui

The default stack holds up. egui is immediate mode, which suits a custom terminal
grid widget, draws through a single GL context with no WebView, and has a
headless test harness for screenshot tests. iced would look similar but costs more
to get a custom grid widget through its layout system.

## 2026-11-03: One engine thread owns all of PuTTY

PuTTY keeps global timers, callback queues and socket tables, so it cannot run
one instance per thread. All sessions share a single thread running PuTTY's own
`cli_main_loop`. Other threads post commands and wake it through a pipe (Unix) or
event handle (Windows).

## 2026-11-03: PuTTY is built by its CMake and linked as its plink target is

`build.rs` runs CMake on the unmodified submodule, building the `plink` target
because it depends on every library the shim needs and proves PuTTY's own link
line works at the pinned commit.

## 2026-11-03: The shim replaces PuTTY's storage.c at link time

The shim defines every function in PuTTY's `storage.h`, so the linker never pulls
the platform `storage.c`. PuTTY therefore never reads the Windows registry or
`~/.putty`, which portable mode requires. Settings arrive as key/value pairs in
PuTTY's saved-session vocabulary, parsed by PuTTY's own `load_open_settings`. That
keeps defaults and validation in PuTTY and makes imported PuTTY sessions trivial.
No random seed file is kept; PuTTY seeds from the OS on each start.

## 2026-11-03: Refusing an algorithm means declining PuTTY's warning

PuTTY appends any class missing from a preference list, so a list cannot exclude
anything. With legacy off, legacy classes sit below the `WARN` marker and the
application declines the resulting prompt. With legacy on, `WARN` goes last. No
coverage is lost, because every class stays reachable by switching the toggle.

PuTTY has no preference setting for MACs: its list is fixed in `ssh/transport2.c`.
FR-004 is narrowed accordingly, and MACs are reported but not configurable, since
exposing them would require patching PuTTY.

## 2026-11-03: Failure reports use a read-only probe connection

PuTTY's "Couldn't agree" message lists only the one class that failed, and a
declined weak algorithm is not reported as a mismatch at all. To show everything
the device offered, a failure triggers a short second TCP connection that reads
the server's version line and plaintext KEXINIT and then hangs up. This is a
display-only decoder, not a second SSH implementation. It cannot run through a jump
host, so those failures fall back to PuTTY's message. Packet logging was rejected
because it would mean writing session traffic to a file.

## 2026-11-03: Legacy test server is OpenSSH 6.7 from Debian jessie

It is the newest packaged server with SSH-1, ssh-dss, arcfour256, group1-sha1 and
SHA-1-only ssh-rsa, so no source build is needed. PuTTY's SSH-2 code has
`arcfour128` and `arcfour256` but not plain `arcfour`, which the test suite uses as
its natural "no common cipher" case.

## 2026-11-03: Linux builds on Ubuntu 22.04

The UI needs glibc to load X11 and Wayland at runtime, so a musl static binary is
not an option. Building on the oldest supported runner gives the widest glibc
compatibility. Windows links the C runtime statically for the same reason.

## 2026-11-03: Settings live in an inspector, commands in a palette

There is no settings dialog or toolbar. A right-hand inspector opens for one node at a time
and shows each value's source; a command palette reaches everything else. Theme files are
imported by dropping them on the window, which avoids a file dialog dependency.

## 2026-11-03: Ctrl+C and friends go to the device

egui's window layer turns Ctrl+C, X and V into clipboard events and swallows the key. A
network engineer needs those as control characters, so the view maps them back. Copy and paste
are Ctrl+Shift+C and V, or Cmd on macOS. One known limit: Ctrl+V with an empty clipboard sends
nothing, because egui emits no event for it.
