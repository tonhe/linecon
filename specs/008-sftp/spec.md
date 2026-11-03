# Feature Specification: SFTP Pane

**Status**: Draft | **Created**: 2026-11-03

## User stories

1. **Move files (P1)**: From an open SSH session, browse the device's files, download and upload, without
   a separate login dialog. Think config backups and IOS images.
2. **Basic housekeeping (P2)**: Make folders, rename and delete.

## Requirements

- **FR-001** The pane opens on request only and shows nothing by default.
- **FR-002** It connects with the session's own settings, so legacy algorithms, identity and jump host
  all apply, and PuTTY carries the connection.
- **FR-003** Directory listing, download, upload, mkdir, rename and delete. Transfers show progress and
  can be cancelled.
- **FR-004** Binary files arrive byte for byte.
- **FR-005** Servers without SFTP, including SSH-1 devices, get a clear message.

## Success criteria

- **SC-001** A Docker test round-trips a binary file against the legacy server.
