# Implementation Plan: SSH Extras

Everything is settings for PuTTY: `ProxyHost`, `ProxyPort`, `ProxyUsername`, `TryAgent` and
`PortForwardings`. The application parses and validates what the user types. Saved passwords stay out
of the jump path, per `docs/DECISIONS.md`.
