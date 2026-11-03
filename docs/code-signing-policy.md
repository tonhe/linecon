# Code signing policy

Windows releases of linecon0 are **not code signed yet**, so Windows warns about them. This page describes how
they will be handled once signing is in place. macOS signing is described in [macos-signing.md](macos-signing.md).

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

It connects only to the hosts a user tells it to. It has no telemetry and no update check. An optional agent server,
off by default, listens on the local machine only.
