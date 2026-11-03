# Status

## Done
- PuTTY shim and engine: SSH, Telnet, serial and raw through one event API.
- CI builds and tests macOS arm64/x64, Linux x64/arm64 and Windows x64.
- Releases are listed at https://github.com/tonhe/linecon/releases, newest first.
- Legacy test on both Linux runners against a Docker OpenSSH 6.7: group1-sha1, 3des-cbc,
  arcfour256, hmac-md5, ssh-dss, SHA-1 ssh-rsa, 1024-bit host keys and SSH-1.
- Negotiation failures report offered versus allowed.
- Session tree with inheritance and provenance, config persistence, portable mode.
- Terminal window: sidebar, tabs, splits, palette, inspector, cards, themes and import, session
  logs, broadcast, search, auto-reconnect. Tested headlessly only.

- Saved identities: username, key file, agent and password, inherited through folders. Passwords
  use the OS store, or a passphrase-encrypted file in portable mode, and are answered once.

- Jump hosts through PuTTY's SSH proxy, with each hop's legacy settings, port forwards (local, remote,
  dynamic) and ssh-agent login, tested against the Docker server. Changed host keys block.

- Imports from ~/.ssh/config, PuTTY and KiTTY (registry, .reg, portable files) and SecureCRT, by palette
  or by dropping files on the window. Stored passwords are not imported. Snippets with folder scope.

- The legacy toggle now reaches SSH-1-only devices by itself. Auto-reconnect retries real drops only.

- Serial port discovery and console presets. Algorithm order chosen by the user is what PuTTY negotiates.

- SFTP file browser on request (Ctrl+Shift+S): list, download, upload by drop, mkdir, rename, delete.

- MCP server: off by default, loopback with a token, per-pane read or control that is never saved.

- Snippet sharing by exported file.

- First real run on macOS: the window opens, and key login worked, probably through the agent. Fixes from that are in
  `specs/011-first-real-use`.

- Session export and import in a documented format, with folders and optional logins.

- Tested against a real IOS XE 17.18.2 router. RSA with SHA-2 connects by default; SHA-1 ssh-rsa needs legacy.

- Checked with no flags against real IOS XE 17.18.2, Arista cEOS 4.36 and vJunos 26.2, in the CLI and through the
  window's code path. See `tests/lab/README.md`.

## In progress
- Fixes from real use on macOS.

## Next
- Whatever real use of the window turns up.

## Blocked
- Nothing.

## Known gaps
- The window has run on macOS by hand. Linux and Windows have only been exercised headlessly.
- PuTTY reads .ppk key files only, so an OpenSSH key file will not load directly; keys need an agent.
- PuTTY does not read ~/.ssh/config. Use the import command.
- Windows and macOS do not run the Docker legacy test, because hosted runners lack a Linux
  container runtime. They run the loopback tests, and the protocol code is shared.
- Ctrl+V with an empty clipboard sends nothing.
- OS secret stores are only automatically tested on Windows; Keychain has not been confirmed on a real Mac,
  and secret-service needs a logged-in desktop session.
- Pageant and the 1Password agent are untested. A remote forward has no automated test.
- Importers are tested against samples written from the formats, not exports from real installs.
- MCP is tested with its own client code and has not been run against a real MCP client.
- Saved passwords are not used through a jump host, because the prompt may be the jump host's.
- macOS builds are not notarized, so Gatekeeper blocks them until allowed by hand.
