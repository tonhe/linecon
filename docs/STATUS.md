# Status

## Done
- PuTTY shim and engine: SSH, Telnet, serial and raw through one event API.
- CI green on macOS arm64/x64, Linux x64/arm64 and Windows x64.
- Legacy test on both Linux runners against a Docker OpenSSH 6.7: group1-sha1, 3des-cbc,
  arcfour256, hmac-md5, ssh-dss, SHA-1 ssh-rsa, 1024-bit host keys and SSH-1.
- Negotiation failures report offered versus allowed.
- Session tree with inheritance and provenance, config persistence, portable mode.

## In progress
- Terminal UI (feature 003): alacritty_terminal core, egui window, sidebar.

## Next
- Identities and secrets, then tabs and splits, then the requirements list in order.

## Blocked
- Nothing.

## Known gaps
- Windows and macOS do not run the Docker legacy test, because hosted runners lack a Linux
  container runtime. They run the loopback tests, and the protocol code is shared.
- Serial has no automated test yet (task T012 in feature 001).
