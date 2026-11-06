#!/bin/bash
# PuTTY lives in vendor/putty as published, unmodified, at the commit in vendor/PUTTY_COMMIT.
#
#   packaging/putty.sh check            the files are exactly upstream's at that commit
#   packaging/putty.sh status           upstream's newest release, and whether we have it
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
    git -C "$work/upstream" fetch -q "$UPSTREAM" main "+refs/tags/*:refs/tags/*" && break
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
  status)
    # Releases are tagged like 0.83. Security fixes arrive in them first, so a release that
    # is not in the vendored commit is the thing to act on.
    commit=$(cat vendor/PUTTY_COMMIT)
    fetch "$commit"
    latest=$(git -C "$work/upstream" tag -l '[0-9]*.[0-9]*' | grep -E '^[0-9]+\.[0-9]+$' |
      sort -V | tail -1)
    behind=$(git -C "$work/upstream" rev-list --count "$commit..FETCH_HEAD")
    echo "vendored: $commit ($behind commits behind upstream main)"
    echo "latest release: $latest"
    if git -C "$work/upstream" merge-base --is-ancestor "$latest" "$commit"; then
      echo "release $latest is included"
    else
      echo "NEW RELEASE: $latest is not in the vendored commit"
      exit 3
    fi
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
    sed -n '2,8p' "$0"
    exit 2
    ;;
esac
