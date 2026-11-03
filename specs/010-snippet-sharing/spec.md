# Feature Specification: Snippet Sharing

**Status**: Draft | **Created**: 2026-11-03

- **FR-001** Export the snippets in scope for the focused session to a small readable file.
- **FR-002** Dropping such a file on the window adds its snippets to the selected folder, or globally.
- **FR-003** Duplicates by name and text are skipped, and nothing existing is replaced.

**SC-001** Round-trip, duplicate and bad-file cases are unit tested.
