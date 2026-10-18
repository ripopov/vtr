#!/usr/bin/env bash
set -euo pipefail
build_dir=
prefix=
profile=Release
while (($#)); do
    case $1 in
        --target-dir|--prefix)
            (($# >= 2)) || { echo "$1 requires an argument" >&2; exit 2; }
            if [[ $1 == --target-dir ]]; then build_dir=$2; else prefix=$2; fi
            shift 2 ;;
        --debug) profile=Debug; shift ;;
        -h|--help)
            echo "Usage: vtr-cpp/install.sh --target-dir DIR --prefix DIR [--debug]"
            exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done
[[ -n $build_dir && -n $prefix ]] || { echo '--target-dir and --prefix are required' >&2; exit 2; }
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
cmake -S "$root" -B "$build_dir" -DCMAKE_BUILD_TYPE="$profile" -DBUILD_TESTING=OFF -DCMAKE_INSTALL_LIBDIR=lib
cmake --build "$build_dir" --parallel "${JOBS:-2}"
cmake --install "$build_dir" --prefix "$prefix"
