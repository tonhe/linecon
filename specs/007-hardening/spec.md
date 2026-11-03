# Feature Specification: Gaps Found by Analysis

**Status**: Draft | **Created**: 2026-11-03

Comparing the brief with what was built turned up these.

## Requirements

- **FR-001** With legacy on, a device that only speaks SSH-1 connects, without the user choosing a
  protocol version.
- **FR-002** Auto-reconnect retries when a working connection drops. It does not retry after the remote
  session exits normally, after the user rejects or cancels something, or when it never got connected.
- **FR-003** A retry after a drop keeps retrying while the device is unreachable, with backoff.
- **FR-004** Serial sessions offer the ports actually present, and presets for common console cables.
- **FR-005** Scrollback search has automated coverage.
- **FR-006** Algorithm order chosen in the UI reaches PuTTY, checked against a real negotiation.
