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

`build.rs` runs CMake on the unmodified vendored copy, building the `plink` target
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

## 2026-11-03: Encrypted secret file uses Argon2id and ChaCha20-Poly1305

Standard RustCrypto implementations rather than anything from PuTTY, which has no API for this.
The header is authenticated, its cost parameters are capped so a tampered file cannot demand
absurd memory, and a wrong passphrase looks the same as tampering. These only protect local files.

## 2026-11-03: macOS ships as a zipped .app as well as a bare executable

A bare Mach-O file is awkward on a Mac: Finder cannot treat it as an app, and it loses its
executable bit inside an Actions artifact zip. The .app is still one download, and the program is
the same single binary, so constraint 2 holds. Linux and macOS archives preserve the executable bit.
The bundle is ad-hoc signed only. Notarization needs a paid Apple Developer account, so Gatekeeper
still blocks a downloaded copy until the user allows it. In portable mode the marker and data sit
beside the .app, because writing inside a signed bundle would break it.

## 2026-11-03: Windows secrets use Credential Manager

The brief says DPAPI. Credential Manager encrypts with DPAPI and is the supported interface to it,
and the same library covers Keychain and secret-service. On Linux it talks D-Bus in pure Rust,
because linking libdbus would add a runtime dependency to the portable binary.

## 2026-11-03: Saved passwords answer once and skip jump hosts

A wrong saved password could otherwise lock an account by retrying, so the second prompt goes to the
user. With a jump host configured the prompt may belong to the jump host, and answering would send
the target's password to the wrong machine, so saved passwords stay out of that path for now.

## 2026-11-03: The jump host's settings are served as a PuTTY saved session

PuTTY's SSH proxy builds its Conf by loading a saved session named after the proxy host, and otherwise
uses defaults, which would drop legacy algorithms on the hop. The shim's storage layer answers that
lookup with settings the application supplies, prefixed `Jump.`. PuTTY stays unpatched. `ProxyLocalhost`
is always set, so a target that looks local cannot skip the jump. A mistyped jump host fails the
connection rather than going direct.

## 2026-11-03: The Rust toolchain is pinned

Floating stable meant CI could run a newer clippy than a developer's machine and fail on code that
looked clean. `rust-toolchain.toml` and the workflow name the same version, bumped deliberately.

## 2026-11-03: SFTP file requests are encoded in Rust, over PuTTY's SSH

PuTTY's sftp.c waits for replies by pumping the event loop inside its receive call, so running it on the
shared engine thread would freeze every other session. A helper process would avoid that at the cost of a
second copy of the engine per transfer. Instead PuTTY opens the `sftp` subsystem and carries everything
SSH, and a small codec handles SFTP v3 requests. That is a file protocol above SSH, so constraint 1 holds.

## 2026-11-03: Agent access is opt-in at every level

The brief lists an MCP server so an agent can drive open sessions. These are live sessions on network gear, and
text from a device can try to instruct an agent, so the server is off by default, grants are per pane and never
saved, and the tools for opening connections or reading secrets do not exist. The window re-checks access when it
applies queued input, so withdrawing a grant wins over anything in flight.

## 2026-11-03: Quick connections are kept once they work

Connections from the quick connect box go in a "Quick sessions" folder, but only after they connect, so
typos do not pile up. Reconnecting reuses the entry and moves it to the top, and only the newest 25 are kept
there. Dragging one to another folder keeps it for good. Passwords are saved only by asking.

## 2026-11-03: Session bundles are versioned TOML, without passwords

No agreed standard exists for terminal client sessions, so linecon0 defines a small documented format using
the same types as its config file. Export files get shared, so passwords are never included, not even encrypted.
A folder export folds in what it inherited, and an import never touches existing entries.

## 2026-11-03: RSA with SHA-2 is modern, and SHA-1 ssh-rsa is legacy

PuTTY has one preference class for rsa-sha2-512, rsa-sha2-256 and ssh-rsa, so a list cannot separate them, and
treating the class as legacy refused current gear such as IOS XE, which has only an RSA key signing with SHA-2.
The class stays below the warning line, and the warning PuTTY raises names the exact algorithm it chose, so the
application accepts it for rsa-sha2-* and declines it for ssh-rsa. PuTTY stays unpatched. A consequence is that
modern keys are still preferred over RSA, as before.

## 2026-11-03: We send our own SSH version string

PuTTY's placeholder for unofficial builds, `-Unidentified-Local-Build`, appears in device logs. The shim defines
the version strings itself, as `linecon_<version>`, so `utils/version.c` is never linked.

## 2026-11-03: A saved password belongs to one session unless shared on purpose

Quick connections reused any login with the same username, which would have sent one device's saved password to
another. Usernames like admin say nothing about passwords. Automatically saved sessions now only share a login
holding no password, and remembering a password never changes a login that other sessions use. Sharing is still
possible by choosing the same login in settings.

## 2026-11-03: Signing runs only for tags, and half-configured signing fails

The signing secrets reach only the packaging step of tag builds, in a throwaway keychain. Having some secrets but
not others stops the release rather than silently shipping unsigned. With none set, builds stay ad-hoc.

## 2026-11-03: Two renderers, and startup failures are shown

Cloud desktops and many VMs have no GPU, and Windows offers them no usable OpenGL, so a window that needs OpenGL
just fails to appear. Both OpenGL and wgpu are built in. Windows starts with wgpu, which has a software Direct3D
adapter, and everything else starts with OpenGL. Each falls back to the other, at the cost of a larger binary.
With no console on Windows, startup failures also go to a message box and to `linecon-error.log` beside the config.

## 2026-11-03: The Windows build is a console program that hides its console

As a GUI-subsystem program, a shell does not wait for it, so an interactive `linecon ssh` competed with cmd for
the keyboard and output landed after the prompt. A single executable cannot be both, so it is a console program,
and it releases its console when started from Explorer. The cost is a console window that may flash on a double
click. Attached to a shell, the window's errors stay visible.

## 2026-11-04: Updates are opt-in and signed with their own key

The program asks once at first launch, because it runs on client networks where an unexpected call out needs a reason.
A SHA-256 file on the same release page proves nothing if the release is tampered with, so CI signs a manifest with an
Ed25519 key held only in a repository secret, and the public key is built in. That key can sign anything, so anyone
who can change the release workflow could use it: the protection is against a tampered release or download, not a
compromised repository. macOS installs only notarized updates and otherwise links to the download page.

## 2026-11-04: SecureCRT passwords are imported, decrypted

Imports used to leave every saved password behind. SecureCRT users moving sessions would otherwise retype all of them,
and the encryption is only as strong as the configuration passphrase, usually empty. They are decrypted once and go
only to the secret store, never to the config file or a bundle. An inline password gets its own login so it cannot be
sent to another device, and a SecureCRT credential stays shared because its owner chose that. The older `Password`
format is not read, so those sessions ask.

## 2026-11-04: A panic ends the program, except that a pump says so first

Most locks unwrap, and a panic inside a C callback aborts. Carrying on with state that may be half-changed is worse for a
tool connected to live devices. A pane's pump thread marks its pane closed as it goes, so a panic there cannot leave a
pane looking connected.

## 2026-11-05: New host keys can be trusted automatically, changed ones never

Trust and remember is the default answer for a host seen for the first time, and a global setting, off by default, takes
that answer without asking and prints the fingerprint in the terminal. Anything that contradicts what is remembered, a
different key or a new key type on a known host, still asks, and there Enter means Cancel. Telling these apart asks the
key store rather than reading PuTTY's wording, since a change of wording must not turn a warning into a silent accept.

## 2026-11-05: The menus are the list of shortcuts

The window's key handling reads the same entries the menus show, so a shortcut cannot exist without its menu item. On
macOS the menu takes the keys, and a key it takes never reaches the window, so each runs once. Copy and paste are the
exception: text fields only respond to the keys themselves, so the menu names those shortcuts in the label and leaves
the keys alone.

## 2026-11-05: New sessions go in the selected folder, or the top level

A selected session says nothing about where a new one belongs, and following it put sessions inside collapsed folders
where nobody could find them. Only a selected folder decides, the dialog shows and can change the choice, and the
result is revealed. There is no special default folder: the top level is that, and clicking empty space returns to it.
A new session is a dialog with a Save button and nothing exists until then. Editing an existing one still saves as you
go, shortly after each change, and the panel says so.

## 2026-11-05: With no GPU the program paints its own window

Cloud desktops and jump boxes are where this tool gets used, and they have no GPU. Windows offers them Direct3D emulated
in software, and on a four core machine that took every core to reach 8 frames a second. Building a frame took under a
millisecond, so no tuning of the program could help. Those machines now use a small rasterizer and a plain window
surface, which reached 30 frames a second on the same machine at a few percent of one core. The ready-made backend for
this tracks an older egui, and what egui draws is simple enough to own. On this path the session manager is a panel in
the main window, and there is no screen reader support yet.

## 2026-11-05: PuTTY is vendored, not a submodule

A release failed because PuTTY's git server dropped the connection while CI fetched the submodule. PuTTY now sits in
`vendor/putty` as upstream published it, with the commit in `vendor/PUTTY_COMMIT`, so building needs nothing but this
repository. It is still never patched: `packaging/putty.sh check` compares git's hash of the directory with upstream's
tree for that commit, in a workflow of its own so that a release never waits for that server, and
`packaging/putty.sh update <commit>` is the only way it changes. A separate mirror repository was considered and not
wanted.

## 2026-11-06: With no GPU, the session manager stays a panel

Screen readers now work on the no-GPU window: it connects egui's accessibility tree the same way eframe does. The
session manager there is still a panel inside the main window, not a window of its own. A second window would mean the
runner managing several windows, surfaces and damage buffers, for a machine where every pixel is already painted by the
processor. The panel does everything the window does, and nothing about it depends on being separate.

## 2026-11-06: Window state lives beside the config

Window size and place, the selected row, the sidebar and which saved sessions were open are kept in `window.ron`,
`window-cpu.json` and `window-state.json` next to `config.toml`, not in it. They describe one machine's window, and the
config may be copied or synced between machines. Sessions that were open are offered on the empty window, and never
reconnect on their own: a device should not see a login nobody asked for.
