# Feature Specification: SSH Extras

**Status**: Draft | **Created**: 2026-11-03

## User stories

1. **Jump hosts (P1)**: Reach a device through a bastion, set on a folder and inherited, with the
   jump host's own username and port. Both hops may need legacy algorithms.
2. **Agents (P1)**: Log in with keys from ssh-agent, Pageant or the 1Password agent.
3. **Port forwards (P2)**: Local, remote and dynamic forwards per session or folder.
4. **Known hosts travel (P2)**: Host keys sit with the config, and a changed key blocks by default.

## Requirements

- **FR-001** Jump host accepts `[user@]host[:port]` and uses PuTTY's SSH proxy, never a second
  implementation.
- **FR-002** Jump host prompts and host key questions reach the user, labelled as the jump host's.
- **FR-003** Agent use follows the identity. PuTTY's agent client does the work.
- **FR-004** Port forwards are entered as `L8080 host:80`, `R9000 host:22` and `D1080`, inherit like
  other settings, and map to PuTTY's PortForwardings setting.
- **FR-005** Invalid forwards are reported in the inspector, before connecting.

## Success criteria

- **SC-001** A Docker test connects through one legacy server to another.
- **SC-002** A Docker test passes traffic through a local forward and a dynamic forward.
- **SC-003** A test with a local ssh-agent logs in using an agent key on Unix.
