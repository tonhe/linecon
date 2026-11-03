# Feature Specification: MCP Server

**Status**: Draft | **Created**: 2026-11-03

An AI agent can drive sessions the user has opened, under the user's control. These are live sessions on
network equipment, so a mistake or a prompt injection in device output can take down a network. Access is
therefore opt-in at every level.

## Requirements

- **FR-001** The server is off by default. Enabling it is a deliberate action.
- **FR-002** Every pane starts with no access, and access is never saved. The user grants read or control
  to one pane at a time.
- **FR-003** A pane with agent access is marked on screen.
- **FR-004** Local connections only, authenticated with a random token in a file readable by the user.
- **FR-005** Agents see only panes with access, and cannot open connections, read saved passwords or change
  settings.
- **FR-006** Input is refused while the pane is waiting on a password, host key or other question.
- **FR-007** `linecon mcp` bridges MCP over stdio to the running window.

## Success criteria

- **SC-001** Tests show a pane with no access cannot be listed, read or written, and read access cannot write.
- **SC-002** A wrong token gets nothing.
