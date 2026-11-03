# Implementation Plan: Session Export and Import

A TOML bundle using the same types as the config file, documented in `docs/session-format.md`. Parsing lives in `linecon-config::import`. Exports go to the downloads folder and imports arrive by dropping the file on the window, as with other imports.
