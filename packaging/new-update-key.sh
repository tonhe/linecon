#!/bin/sh
# Creates the update signing key. The private key goes to a repository secret and to
# the backup file you name, and is never printed. Prints the public key.
# usage: packaging/new-update-key.sh <backup file>
set -eu
backup="$1"
if [ -e "$backup" ]; then
  echo "$backup already exists; not overwriting a key" >&2
  exit 1
fi
umask 077
openssl genpkey -algorithm ed25519 -out "$backup"
gh secret set UPDATE_SIGNING_KEY -R tonhe/linecon < "$backup"
echo "Secret set. Keep $backup somewhere safe and offline."
echo "Public key, which is safe to share:"
openssl pkey -in "$backup" -pubout -outform DER | tail -c 32 | od -An -tx1 | tr -d ' \n'
echo
