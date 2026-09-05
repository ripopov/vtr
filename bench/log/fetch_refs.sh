#!/usr/bin/env sh
# Fetches the four logging systems the log benchmark compares against into ext/
# (shallow, pinned to the commits the published numbers were measured with).
# They are plain clones, not submodules: nothing here builds them, and the
# files stay out of version control (.gitignore). Re-running is a no-op for a
# directory that already exists.
#
#   sh bench/log/fetch_refs.sh          # then: python3 bench/run.py log
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
fetch() {
    name=$1; url=$2; sha=$3
    dir="$ROOT/ext/$name"
    if [ -d "$dir" ]; then
        echo "$name: present"
        return
    fi
    echo "$name: fetching $sha from $url"
    git init -q "$dir"
    git -C "$dir" remote add origin "$url"
    git -C "$dir" fetch -q --depth 1 origin "$sha"
    git -C "$dir" checkout -q FETCH_HEAD
}
fetch clp     https://github.com/y-scope/clp            2a5fbee5d6c6635aa3a4c354317b57a865b96bd2
fetch NanoLog https://github.com/PlatformLab/NanoLog    2a94d70f9d1db4da416053b1b926387fa068a59b
fetch binlog  https://github.com/morganstanley/binlog   fd20f5a7592f040141ccbdcc737386235bc8b0cf
fetch quill   https://github.com/odygrd/quill           925b2960034627f2443919f11f8369b59ae8eea7
