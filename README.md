# linecon0

`line con 0`

[![CI](https://github.com/tonhe/linecon/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/tonhe/linecon/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/tonhe/linecon?include_prereleases&sort=date&label=release)](https://github.com/tonhe/linecon/releases)
[![Downloads](https://img.shields.io/github/downloads/tonhe/linecon/total)](https://github.com/tonhe/linecon/releases)
[![License: MIT](https://img.shields.io/github/license/tonhe/linecon)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-blue)](#install)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org)
[![Protocol engine: PuTTY](https://img.shields.io/badge/protocol%20engine-PuTTY-informational)](https://www.chiark.greenend.org.uk/~sgtatham/putty/)
[![GitHub stars](https://img.shields.io/github/stars/tonhe/linecon?style=flat)](https://github.com/tonhe/linecon/stargazers)

A portable SSH, Telnet and serial client for network engineers. It uses PuTTY
as its protocol engine, so it can still talk to old hardware that needs SSH-1,
group1-sha1, 3des-cbc, arcfour, ssh-dss or SHA-1 ssh-rsa.

Each platform gets a single executable with no installer, bundled runtime or
WebView: macOS (arm64, x64), Linux (x64, arm64) and Windows (x64).

![Terminal and session sidebar](docs/screenshots/main.png)

The default view is a terminal and a collapsible session sidebar. Everything else
appears when you ask for it.

Folders carry settings and children inherit them, and the inspector shows where each
value comes from:

![Settings showing inherited values](docs/screenshots/inspector.png)

When a device and your settings cannot agree, you see exactly what it offered against
what was allowed, and can allow legacy algorithms with one click:

![Negotiation failure report](docs/screenshots/failure.png)

A file browser opens on request, on the same connection settings:

![SFTP file browser](docs/screenshots/files.png)

Early development; see [docs/STATUS.md](docs/STATUS.md).

## Install

Downloads are on the [releases page](https://github.com/tonhe/linecon/releases).

- **macOS**: unzip `linecon0-macos-*.app.zip` (`arm64` for Apple silicon, `x64` for Intel). Until releases
  are notarized, macOS blocks it at first. Either run
  `xattr -dr com.apple.quarantine linecon0.app`, or try to open it once and then choose
  Open Anyway under System Settings > Privacy & Security. `linecon-macos-*-cli.tar.gz` is the same
  program as a bare executable for terminal use, with no app icon.
- **Linux**: unpack `linecon-linux-*.tar.gz` and run `./linecon`.
- **Windows**: run `linecon-windows-x64.exe`. SmartScreen may warn, since it is unsigned. See the
  [code signing policy](docs/code-signing-policy.md). A console window may flash when starting it from Explorer.

### Code signing

linecon0 is applying to use the [SignPath Foundation](https://signpath.org) for free code signing of its Windows
releases. Until that is approved, Windows releases are unsigned. See the
[code signing policy](docs/code-signing-policy.md), which also covers privacy: the program has no telemetry and
connects only where you tell it to.

## Use

Run `linecon` with no arguments for the window, or with arguments for a command-line
client (`linecon --help` lists them). A file named `linecon.portable` next to the binary
keeps config, known hosts and logs in a `linecon-data` folder beside it.

| Shortcut (Cmd on macOS) | |
|---|---|
| Ctrl+Shift+P | Command palette |
| Ctrl+Shift+B | Hide or show the sidebar |
| Ctrl+Shift+I | Settings for this session |
| Ctrl+Shift+S | File browser |
| Ctrl+Shift+D / E | Split right / down |
| Ctrl+Shift+W | Close pane |
| Ctrl+Shift+F | Search scrollback |
| Ctrl+Shift+C / V | Copy / paste (right-click also pastes, and selecting copies) |
| Ctrl+Shift+R | Reconnect |
| Alt+arrows | Move between panes |

Drop an iTerm2 `.itermcolors` or Windows Terminal `.json` scheme on the window to import it.

## Moving sessions

Export everything from the command palette, or one folder by right-clicking it, to a readable file described in
[docs/session-format.md](docs/session-format.md). Drop that file on another window to import it. Folders and
inherited settings come along, usernames and key paths only if you ask, and passwords never.

## Agent access

linecon0 can act as an [MCP](https://modelcontextprotocol.io) server so an AI agent can work in sessions you
choose. These are live sessions on real equipment, and output from a device could contain text that tries to
instruct the agent, so everything is opt-in:

- The server is off until you start it from the command palette.
- Each pane starts with no access. You grant read or control one pane at a time, and it is never saved.
- A pane with access is marked on screen, and input is refused while it is waiting on a password.
- Agents cannot open connections, read saved passwords or change settings.

Point your MCP client at the command `linecon mcp`.

## Build

    git clone --recursive https://github.com/tonhe/linecon
    cargo build --release -p linecon

Needs Rust, CMake and a C compiler. Tests use Docker for the legacy SSH server where available.

## License

MIT. See [LICENSE](LICENSE). PuTTY is MIT licensed by Simon Tatham and contributors.
JetBrains Mono and Inter are under the SIL Open Font License; see `crates/linecon/assets/fonts`.
