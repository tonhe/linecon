# Feature Specification: Saved Identities and Secrets

**Status**: Draft | **Created**: 2026-11-03

## User stories

1. **Reuse a login (P1)**: An engineer defines an identity once (username, plus key file, agent
   key or password) and applies it to a session or folder. Children inherit it.
2. **Never retype or expose (P1)**: A saved password is answered automatically, is encrypted at
   rest, and never appears in the config file, logs or diagnostics.
3. **Portable (P2)**: In portable mode, secrets live in a passphrase-encrypted file that travels
   with the binary.

## Requirements

- **FR-001** Identities have a name, username, optional key file, optional agent key and optional
  saved password. Sessions and folders refer to them by id.
- **FR-002** Normal mode stores secrets in the OS store: macOS Keychain, Windows DPAPI, Linux
  secret-service.
- **FR-003** If no OS store is available, or in portable mode, use a passphrase-encrypted file. The
  passphrase is asked for once per run.
- **FR-004** A saved password answers only the server's password prompt, and a failed attempt falls
  back to asking the user rather than retrying.
- **FR-005** Key file, agent use and username reach PuTTY as settings. The secret never does.
- **FR-006** The inspector shows which identity applies and which folder it came from.

## Success criteria

- **SC-001** The Docker test logs in with a saved password with no user interaction.
- **SC-002** A test confirms the config file and the encrypted file contain no secret in plaintext.
- **SC-003** A test confirms a wrong passphrase is refused and does not damage the file.
