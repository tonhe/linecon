# Implementation Plan: Saved Identities and Secrets

Identities go in `linecon-config`, stored in the config file without secrets. A `SecretStore` trait
gets per-OS implementations plus an encrypted file: Argon2id key derivation and ChaCha20-Poly1305
from RustCrypto crates. These protect local files only and carry no session traffic, so principle I
is unaffected. Pane answers password prompts from the store and clears its copy afterwards.
