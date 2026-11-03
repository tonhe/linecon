# Feature Specification: Session Export and Import

**Status**: Draft | **Created**: 2026-11-03

## Requirements

- **FR-001** Export everything or one folder to a documented, versioned, readable file.
- **FR-002** Folder structure and inheritable settings survive the round trip, and values a folder inherited
  from outside the export are kept.
- **FR-003** Logins are optional. Without them the file holds no usernames or key paths.
- **FR-004** Passwords are never exported, whatever the options.
- **FR-005** Importing never changes existing entries. Everything lands in a new folder, and logins matching
  an existing username and key are reused.
- **FR-006** A file from a newer format version is refused with a clear message.
