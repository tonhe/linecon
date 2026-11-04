#!/bin/sh
# Writes the signed update manifest for a release. It lists the hash of every file in
# <dist>, and the program checks it against the public key built into it.
# usage: UPDATE_SIGNING_KEY=<PEM> packaging/sign-update.sh <dist> <version> <notarized: yes|no>
set -eu
dist="$1"
version="${2#v}"
notarized="$3"
# Once programs carry a public key, a release that cannot update them is a mistake.
if [ -z "${UPDATE_SIGNING_KEY:-}" ] && [ -s packaging/update-public-key.txt ]; then
  echo "programs expect signed updates, and UPDATE_SIGNING_KEY is missing" >&2
  exit 1
fi
if [ -z "${UPDATE_SIGNING_KEY:-}" ]; then
  echo "UPDATE_SIGNING_KEY is not set; this release will not offer itself as an update"
  exit 0
fi
manifest="$dist/linecon-update.txt"
{
  echo "linecon-update-1"
  echo "version $version"
  echo "notarized $notarized"
  for file in "$dist"/*; do
    name=$(basename "$file")
    case "$name" in
      linecon-update.*) continue ;;
    esac
    echo "file $name $(sha256sum "$file" | cut -d' ' -f1)"
  done
} > "$manifest"
umask 077
key=$(mktemp)
trap 'rm -f "$key"' EXIT
printf '%s\n' "$UPDATE_SIGNING_KEY" > "$key"
openssl pkeyutl -sign -rawin -inkey "$key" -in "$manifest" -out "$dist/linecon-update.sig"
