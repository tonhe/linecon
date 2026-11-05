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
  dynamic) and ssh-agent login, tested against the Docker server. Changed host keys block. New hosts can be trusted
  automatically by a global setting; a changed key still asks.

- Machines with no GPU paint with the processor directly. Forced with `LINECON_RENDERER=cpu`.

- Imports from ~/.ssh/config, PuTTY and KiTTY (registry, .reg, portable files) and SecureCRT, by palette
  or by dropping files on the window. SecureCRT passwords go to the secret store, and other tools' stored passwords are not imported. Snippets with folder scope.

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

- The real window is driven with real input against real devices by `tests/gui`. 16 flows pass on Linux.

- Updates from GitHub releases: opt-in checks, an Ed25519-signed manifest, a built-in key, and rollback. Tested against
  a local host. The key is in place from v0.0.1-alpha.18, and no real update between two releases has run yet.
- macOS releases are signed and notarized from v0.0.1-alpha.15. Fetching the ticket failed on one CI machine in two
  runs, cause unknown, and rerunning the job on a fresh machine passed.

## In progress
- Fixes from real use on macOS.

## Next
- Whatever real use of the window turns up.

## Blocked
- Windows signing: the SignPath Foundation declined for now, wanting more public visibility. See #5.
- macOS signing waits on Developer ID secrets in the repository. See `docs/macos-signing.md`.

## Known gaps
- The window is driven automatically on Linux and has been run on macOS by hand. Windows has only been run headlessly.
- OpenSSH private keys are converted once for PuTTY, kept beside the settings. Keys PuTTY cannot import, such as hardware-backed
  ones, still need an agent.
- PuTTY does not read ~/.ssh/config. Use the import command.
- Windows and macOS do not run the Docker legacy test, because hosted runners lack a Linux
  container runtime. They run the loopback tests, and the protocol code is shared.
- Ctrl+V with an empty clipboard sends nothing.
- The system clipboard on macOS awaits confirmation on a build that has the clipboard fix.
- OS secret stores are only automatically tested on Windows. Keychain is confirmed by hand on a real Mac: a saved
  login appears under the service `linecon0` in the login keychain. secret-service needs a logged-in desktop session.
- Pageant and the 1Password agent are untested. A remote forward has no automated test.
- Importers are tested against samples written from the formats, not exports from real installs.
- MCP is tested with its own client code and has not been run against a real MCP client.
- Saved passwords are not used through a jump host, because the prompt may be the jump host's.
