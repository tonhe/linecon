# Implementation Plan: SFTP Pane

PuTTY's sftp.c blocks in its receive call by pumping the event loop, which would stall every session on
the shared engine thread. Instead a session opens with PuTTY's `sftp` subsystem, so all of SSH stays in
PuTTY, and `linecon-engine::sftp` encodes SFTP v3 file requests on top from its own thread.
