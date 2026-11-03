# Feature Specification: Terminal Window

**Status**: Draft | **Created**: 2026-11-03

The default view is a terminal and a collapsible session sidebar, and nothing else.

## Requirements

- **FR-001** Embed alacritty_terminal with true color, xterm-256color and large scrollback.
- **FR-002** Keyboard input reaches the device, including Ctrl+C, Ctrl+Z and Ctrl+^.
- **FR-003** Mouse passthrough, copy on select, multi-line paste confirmation, search.
- **FR-004** Tabs appear only with more than one; panes split and resize.
- **FR-005** Broadcast input to selected panes.
- **FR-006** Sessions tree with drag to move, context menu and quick connect.
- **FR-007** Inspector shows each effective value and its source; overriding and clearing work.
- **FR-008** Host key, password and negotiation failures appear as cards on the pane.
- **FR-009** Curated themes, iTerm2 and Windows Terminal import, light and dark chrome.
- **FR-010** Session logs with timestamps, on by default.
- **FR-011** Auto-reconnect with backoff, off by default.

## Success criteria

- **SC-001** A headless test types into the real window and the bytes reach a TCP peer.
- **SC-002** Every interactive control has an accessible name.
