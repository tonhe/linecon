# Implementation Plan: Engine Foundation

**Spec**: [spec.md](spec.md)

## Technical context

- Rust stable, C99 for the shim, CMake for PuTTY.
- PuTTY pinned as a submodule at `vendor/putty`, unmodified.
- Tests: `cargo test`; Docker for the legacy server on Linux.
- Targets: macOS arm64/x64, Linux x64/arm64, Windows x64, on native runners.

## Constitution check

| Principle | How this plan complies |
|---|---|
| I. PuTTY is the engine | Submodule, own CMake build, thin shim, no patches |
| II. Legacy coverage | Enumeration test against `settings.c`, Docker test per legacy algorithm |
| III. One executable | Static PuTTY, static CRT on Windows, no runtime dependencies added |
| IV. Terminal core and UI | Out of scope here |
| V. Inheritance | Out of scope here |
| VI. Secrets | Prompt answers go straight to PuTTY and are never logged or put in Conf |

## Structure

```text
crates/linecon-putty-sys/   build.rs, C shim, raw bindings
crates/linecon-engine/      safe API: Session, Event, algorithms, negotiation, host keys
crates/linecon/             the binary
tests/docker/legacy-sshd/   legacy-only server image
.github/workflows/ci.yml    five-target matrix, lint, releases
```

## Clarifications

Resolved from the project constraints:

- MACs: PuTTY has no preference setting for them, so FR-004 covers ciphers, key
  exchange and host keys, and MACs are reported only. See `docs/DECISIONS.md`.
- "Disallowed" means below PuTTY's warning line with the warning declined, since
  PuTTY's lists cannot exclude entries.
- Plain `arcfour` does not exist in PuTTY's SSH-2 code. Coverage means `arcfour128`
  and `arcfour256`, plus SSH-1 ciphers.
