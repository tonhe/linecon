#!/bin/sh
# Drives the real window in a virtual desktop against the device lab. See README.md.
# usage: tests/gui/run.sh <linecon binary> <folder for screenshots>
set -e
binary=$(realpath "$1"); out=$(realpath "$2")
here=$(dirname "$(realpath "$0")")
docker build -q -t linecon-gui "$here" >/dev/null
docker run --rm --network host -e LINECON_RENDERER -v "$binary":/bin/linecon:ro -v "$out":/out \
  -v "$here/drive.py":/drive.py:ro linecon-gui python3 /drive.py
