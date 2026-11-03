# Feature Specification: Session Imports and Snippets

**Status**: Draft | **Created**: 2026-11-03

## User stories

1. **Bring my sessions (P1)**: Import from PuTTY and KiTTY (registry and portable files), SecureCRT,
   and `~/.ssh/config`, keeping folders, usernames, keys and jump hosts where the source has them.
2. **Quick commands (P2)**: Save commands on a folder or globally, and run them from the palette in
   any session beneath that scope.

## Requirements

- **FR-001** Imports land in a new folder named after the source and never change existing entries.
- **FR-002** Usernames and key files become shared identities, one per distinct pair.
- **FR-003** Imported passwords are not read, including SecureCRT's stored ones.
- **FR-004** Files are recognised by content, so dropping a file or folder on the window works.
- **FR-005** A broken entry is skipped and counted, and does not stop the rest.
- **FR-006** A snippet has a name and text, and is offered for sessions under its folder. Running it
  honours broadcast.

## Success criteria

- **SC-001** Unit tests cover each format with realistic samples, including odd encodings.
- **SC-002** A palette test shows snippet scope following the tree.
