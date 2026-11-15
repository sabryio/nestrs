#!/usr/bin/env bash
# Path of a crate in the crates.io sparse index: 1/a, 2/ab, 3/a/abc, ab/cd/abcd...
n="$1"; l=${#n}
if   [ "$l" -le 2 ]; then echo "$l/$n"
elif [ "$l" -eq 3 ]; then echo "3/${n:0:1}/$n"
else echo "${n:0:2}/${n:2:2}/$n"; fi
