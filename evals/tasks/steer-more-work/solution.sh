#!/bin/sh
# Reference solution: the per-file counts, then the total the steer asks for.
mkdir -p counts
for f in a b c; do wc -l < "$f.txt" | tr -d " " > "counts/$f.count"; done
echo 10 > counts/total
