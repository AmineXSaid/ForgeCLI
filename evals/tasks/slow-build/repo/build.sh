#!/bin/sh
# A build that takes longer than Bash's default wait (2 minutes).
set -e
echo "compiling 3 sources..."
sleep 150
mkdir -p dist
echo "built from 3 sources" > dist/app.txt
echo "done: dist/app.txt"
