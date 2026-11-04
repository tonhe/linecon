# Feature Specification: Updates From GitHub Releases

**Status**: Draft | **Created**: 2026-11-04

A program that replaces itself is a target: tampering with one release would reach every install.

## Requirements

- **FR-001** The first launch asks once whether to check automatically. Off if declined. A palette command checks on
  request either way. Checks send no identifying data.
- **FR-002** Nothing is replaced without the user choosing to, and restarting says how many sessions it closes.
- **FR-003** Every update is verified against a signed manifest, using a public key built into the program. A failed
  check installs nothing.
- **FR-004** The signed version must be newer than the running one, so an old release cannot be replayed.
- **FR-005** The new program must run and report the expected version before it replaces anything. A failed swap
  restores the old one.
- **FR-006** While this build is a prerelease it follows prereleases. A stable build follows stable releases only.
- **FR-007** macOS installs only notarized updates, and otherwise links to the download page.
- **FR-008** Portable data is untouched, and with no permission to replace the file the user gets the download page.
- **FR-009** With no key configured the feature is absent.

## Success criteria

- **SC-001** Tests reject a tampered manifest, a wrong key, a wrong hash, a downgrade and a path-traversing archive.
- **SC-002** A test replaces a real executable end to end, and a failing new version leaves the old one in place.
