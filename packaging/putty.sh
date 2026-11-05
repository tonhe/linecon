#!/bin/bash
# PuTTY lives in vendor/putty as published, unmodified, at the commit in vendor/PUTTY_COMMIT.
#
#   packaging/putty.sh check            the files are exactly upstream's at that commit
#   packaging/putty.sh update <commit>  replace them with another upstream commit
#
# Only these touch PuTTY's server. Building never does, so its being down cannot stop a release.
set -euo pipefail
cd "$(dirname "$0")/.."
UPSTREAM=https://git.tartarus.org/simon/putty.git
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fetch() {
  git -C "$work" init -q upstream
  # The server serves whole branches, not single commits.
  # It also drops connections now and then.
  for attempt in 1 2 3 4 5; do
    git -C "$work/upstream" fetch -q "$UPSTREAM" main && break
    [ "$attempt" = 5 ] && return 1
    sleep $((attempt * 10))
  done
  git -C "$work/upstream" cat-file -e "$1^{commit}"
}

# Git's own hash of the directory, so file contents, names and modes all count.
tree_of_vendor() {
  local index="$work/index"
  GIT_INDEX_FILE="$index" git read-tree --empty
  GIT_INDEX_FILE="$index" git add -f vendor/putty
  GIT_INDEX_FILE="$index" git write-tree --prefix=vendor/putty/
}

case "${1:-}" in
  check)
    commit=$(cat vendor/PUTTY_COMMIT)
    fetch "$commit"
    want=$(git -C "$work/upstream" rev-parse "$commit^{tree}")
    have=$(tree_of_vendor)
    if [ "$want" != "$have" ]; then
      echo "vendor/putty differs from upstream $commit"
      git -C "$work/upstream" archive "$commit" | tar -x -C "$work" --one-top-level=theirs
      diff -rq "$work/theirs" vendor/putty | head -20 || true
      exit 1
    fi
    echo "vendor/putty is upstream $commit, unmodified"
    ;;
  update)
    commit=${2:?usage: packaging/putty.sh update <commit>}
    fetch "$commit"
    commit=$(git -C "$work/upstream" rev-parse "$commit^{commit}")
    rm -rf vendor/putty
    mkdir -p vendor/putty
    git -C "$work/upstream" archive "$commit" | tar -x -C vendor/putty
    echo "$commit" > vendor/PUTTY_COMMIT
    echo "vendor/putty is now upstream $commit. Run the tests and the lab before committing."
    ;;
  *)
    sed -n '2,7p' "$0"
    exit 2
    ;;
esac
