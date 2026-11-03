# Feature Specification: Engine Foundation

**Feature Branch**: `main`

**Created**: 2026-11-03

**Status**: Draft

**Input**: First milestone: all five targets build and test in CI with the protocol engine linked, and an automated test connects to a server that only allows legacy algorithms.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Connect to legacy-only SSH hardware (Priority: P1)

A network engineer connects to a device that only offers old SSH algorithms and gets a working session, with no reduction in what the engine can negotiate.

**Why this priority**: Compatibility with old hardware is the reason the project exists. If this fails, nothing else matters.

**Independent Test**: Start a containerized SSH server restricted to legacy algorithms, connect through the engine, authenticate, run a command and read its output.

**Acceptance Scenarios**:

1. **Given** a server offering only group1-sha1, 3des-cbc and ssh-rsa with SHA-1, **When** a connection is made with the legacy toggle on, **Then** the session authenticates and command output is received.
2. **Given** a server offering only legacy algorithms, **When** the connection is made with legacy algorithms disallowed, **Then** the connection fails and the error lists what the server offered and what was allowed.
3. **Given** a server presenting an unknown host key, **When** connecting, **Then** the host key decision is delegated to the caller and is never silently accepted.

---

### User Story 2 - Every target builds and tests from day one (Priority: P1)

A maintainer pushes a commit and sees all five targets build, link the engine and pass tests, and a tagged commit publishes a downloadable executable for each.

**Why this priority**: Portability is a hard constraint, and it is cheapest to enforce before there is much code to port.

**Independent Test**: Push a commit and confirm that all five target jobs pass. Push a tag and confirm that five artifacts are attached to the release.

**Acceptance Scenarios**:

1. **Given** a commit on main, **When** CI runs, **Then** macOS arm64, macOS x64, Linux x64, Linux arm64 and Windows x64 all build and pass tests.
2. **Given** a version tag, **When** the release job runs, **Then** one executable per target is attached to the release.
3. **Given** a formatting or lint violation, **When** CI runs, **Then** the build fails.

---

### User Story 3 - Drive all four backends through one interface (Priority: P2)

A developer opens SSH, Telnet, serial and raw sessions through one interface and receives bytes, state changes and prompts as events.

**Why this priority**: The terminal and UI work builds on this interface, so it needs to be stable before they start.

**Independent Test**: A command-line harness opens a session on each backend against a local test peer and echoes data both ways.

**Acceptance Scenarios**:

1. **Given** a local Telnet or raw TCP peer, **When** the harness connects and sends data, **Then** the data is echoed back through the event interface.
2. **Given** a serial port or pseudo-terminal, **When** a session opens at a chosen baud rate, **Then** data flows and a break signal can be sent where the platform supports it.
3. **Given** an open session, **When** the peer disconnects, **Then** the caller receives a closed event with the reason and no resources leak.

### Edge Cases

- The server closes the connection during key exchange.
- The server offers no algorithm in common with the allowed list.
- The password prompt is cancelled by the caller.
- A session is closed while a host key decision is still pending.
- The container runtime is missing on a CI target, in which case the legacy test MUST be reported as skipped and MUST still run on at least Linux x64.
- A serial device is removed while the session is open.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST use the unmodified PuTTY source, pinned as a submodule and built by its own build system, as its only protocol implementation.
- **FR-002**: The system MUST expose ssh, telnet, serial and raw sessions through a single thin C interface callable from the application.
- **FR-003**: The interface MUST deliver received data, connection state, host key decisions, authentication prompts and closure reasons as events.
- **FR-004**: The system MUST allow callers to set the preference order of ciphers, key exchange methods, host keys and MACs, including every legacy algorithm the engine supports.
- **FR-005**: A negotiation failure MUST report the algorithms the server offered and the algorithms that were allowed.
- **FR-006**: An automated test MUST complete an authenticated session against a containerized server restricted to legacy algorithms.
- **FR-007**: CI MUST build and test on all five targets on every push, and fail on formatting or lint violations.
- **FR-008**: Tagged commits MUST attach one self-contained executable per target to a release.
- **FR-009**: Host key verification MUST be delegated to the caller and default to rejection when no decision is supplied.
- **FR-010**: Secrets passed through the interface MUST NOT appear in logs or diagnostics.

### Key Entities

- **Session**: One live connection, with a backend type, a target, an algorithm policy and a lifecycle state.
- **Algorithm Policy**: Ordered allowed lists per algorithm class, plus the legacy toggle state.
- **Session Event**: A data, state, host key, prompt, negotiation failure or closure notification from a session.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All five targets pass in CI on main.
- **SC-002**: The legacy-only test completes an authenticated command round trip on every CI run where it can execute, and on Linux x64 at minimum.
- **SC-003**: A release tag produces five downloadable executables, each of which runs on a clean machine of its platform with no extra software installed.
- **SC-004**: Every negotiation failure names both the offered and the allowed algorithms.
- **SC-005**: Every algorithm in the pinned engine's lists can be selected through the interface, verified by a test that enumerates them.

## Assumptions

- Terminal rendering, the UI and saved configuration are separate features and out of scope here. The executable in this milestone is a command-line harness.
- SSH-1 is verified against a server built specifically for the test, because current server packages no longer include it.
- Containerized tests run on Linux runners. Hosted macOS and Windows runners do not reliably provide a Linux container runtime.
- Serial tests use pseudo-terminals on Unix. Windows serial coverage is limited to build and link until hardware or a virtual port is available.
