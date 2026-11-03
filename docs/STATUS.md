# Status

## Done
- PuTTY shim and engine: SSH, Telnet, serial and raw through one event API.
- CI builds and tests macOS arm64/x64, Linux x64/arm64 and Windows x64.
- Tagged release v0.0.1-alpha.1 published five executables (command-line only at that point).
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

## In progress
- SFTP pane: the engine half is done and tested; the browser UI is next (`specs/008-sftp`).

## Next
- SFTP pane, MCP server, snippet sharing.

## Blocked
- Nothing.

## Known gaps
- The window has only been exercised through a headless harness. No one has yet run it on a real
  desktop on any platform; this machine has no display. Please try a build and report back.
- Windows and macOS do not run the Docker legacy test, because hosted runners lack a Linux
  container runtime. They run the loopback tests, and the protocol code is shared.
- Ctrl+V with an empty clipboard sends nothing.
- OS secret stores are only automatically tested on Windows; Keychain and secret-service need a
  logged-in desktop session.
- Pageant and the 1Password agent are untested. A remote forward has no automated test.
- Importers are tested against samples written from the formats, not exports from real installs.
- Saved passwords are not used through a jump host, because the prompt may be the jump host's.
- macOS builds are not notarized, so Gatekeeper blocks them until allowed by hand.
