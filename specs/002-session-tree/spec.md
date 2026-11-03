# Feature Specification: Session Tree and Inheritance

**Status**: Draft | **Created**: 2026-11-03

## User stories

1. **Organise (P1)**: An engineer groups devices into nested folders by site and role.
2. **Inherit (P1)**: Identity, theme, terminal options, jump host, logging and legacy
   algorithms set on a folder apply to everything beneath it unless overridden.
3. **See where a value comes from (P1)**: For any session, each effective value shows
   whether it is the built-in default, inherited (and from which folder), or overridden
   on the session itself.
4. **Portable (P2)**: The user chooses portable mode (config next to the binary) or
   normal mode (per-user config directory).

## Requirements

- **FR-001** Sessions and folders form a tree of arbitrary depth, with stable ids.
- **FR-002** The nearest ancestor that sets a value wins; a session's own value beats all.
- **FR-003** Resolution returns each value with its source: default, folder (id and name)
  or session.
- **FR-004** Clearing an override restores inheritance.
- **FR-005** The tree round-trips to a human-readable TOML file, and a damaged file is
  reported, never silently replaced.
- **FR-006** Saves are atomic.
- **FR-007** A `linecon.portable` file next to the binary selects portable mode; otherwise
  the per-user config directory is used. Either way the known-hosts file sits with the config.
- **FR-008** Secrets are never stored in the config file. Identities refer to stored secrets
  by id.
- **FR-009** Resolved settings produce PuTTY settings for the engine.

## Success criteria

- **SC-001** Unit tests cover every inherited field at session, folder, nested and default level.
- **SC-002** A legacy toggle on a folder reaches the engine for every session beneath it.
