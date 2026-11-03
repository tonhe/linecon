# Implementation Plan: Gaps Found by Analysis

SSH-1 fallback lives beside the failure reporting: PuTTY reports the version mismatch, then the session
reconnects with `SshProt=0`. No extra probing is needed, and PuTTY still does all the protocol work.
Port listing scans device names and needs no new dependency; libudev would break the portable binary.
