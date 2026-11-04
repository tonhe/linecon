# Code signing policy

Windows releases of linecon0 are **not code signed yet**, so Windows warns about them. The project intends to reapply
to the [SignPath Foundation](https://signpath.org) for free code signing. This page describes how releases will be
signed and approved. macOS signing is described in [macos-signing.md](macos-signing.md).

## Team

| Role | Who |
|---|---|
| Authors, trusted to commit | [tonhe](https://github.com/tonhe) |
| Reviewers | [tonhe](https://github.com/tonhe) |
| Approvers, who approve each signed release | [tonhe](https://github.com/tonhe) |

## What gets signed

Only `linecon.exe`, built from this repository by GitHub Actions on GitHub-hosted runners, from a release tag. Each
release is approved by hand. Nothing built elsewhere is signed.

## Privacy

This program will not transfer any information to other networked systems unless specifically requested by the
user or the person installing or operating it.

It connects only to the hosts a user tells it to. It has no telemetry. It asks GitHub for new releases only if the user agrees at first launch or asks from the palette; that request carries this version number, and GitHub sees the IP address it came from. Nothing is installed without the user choosing to. An optional agent server,
off by default, listens on the local machine only.
