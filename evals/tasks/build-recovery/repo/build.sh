#!/bin/sh
set -e
if [ ! -f build.conf ]; then
  echo "error: build.conf is missing. Create it with a line: name=<app name>" >&2
  exit 2
fi
name=$(sed -n 's/^name=//p' build.conf)
if [ -z "$name" ]; then
  echo "error: build.conf has no name= line" >&2
  exit 3
fi
if [ ! -d src ]; then
  echo "error: src/ directory not found; it must contain main.txt" >&2
  exit 4
fi
mkdir -p dist
cat src/main.txt > dist/app.txt
echo "built $name"
