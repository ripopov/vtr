#!/usr/bin/env bash
# Build and install the Linux static C API into an independent prefix.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: vtr-capi/install.sh --target-dir DIR --prefix DIR [--debug] [--with-dpi] [--private-heap] [--toolchain NAME]

  --target-dir DIR   Cargo output directory (required)
  --prefix DIR       Installation prefix containing lib/, include/, and lib/pkgconfig/ (required)
  --debug            Build the debug profile instead of release
  --with-dpi         Install the optional SystemVerilog/DPI helpers
  --private-heap     Give libvtr a private heap for crash guard recovery
  --toolchain NAME   Select a rustup toolchain (RUSTUP_TOOLCHAIN also works)
EOF
}

die() {
    printf 'vtr-capi install: %s\n' "$*" >&2
    exit 1
}

target_dir=
prefix=
profile=release
with_dpi=false
private_heap=false
toolchain=
while (($#)); do
    case $1 in
        --target-dir|--prefix|--toolchain)
            (($# >= 2)) || die "$1 requires an argument"
            case $1 in
                --target-dir) target_dir=$2 ;;
                --prefix) prefix=$2 ;;
                --toolchain) toolchain=$2 ;;
            esac
            shift 2
            ;;
        --debug) profile=debug; shift ;;
        --with-dpi) with_dpi=true; shift ;;
        --private-heap) private_heap=true; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done

[[ -n $target_dir ]] || die '--target-dir is required'
[[ -n $prefix ]] || die '--prefix is required'
[[ $(uname -s) == Linux ]] || die 'static C API installation is supported on Linux only'
command -v cargo >/dev/null || die 'cargo is required'
command -v rustc >/dev/null || die 'rustc is required'
command -v install >/dev/null || die 'install is required'
if [[ -n $toolchain ]]; then
    command -v rustup >/dev/null || die '--toolchain requires rustup'
    cargo_cmd=(rustup run "$toolchain" cargo)
else
    cargo_cmd=(cargo)
fi

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
mkdir -p -- "$target_dir" "$prefix"
target_dir=$(cd -- "$target_dir" && pwd -P)
prefix=$(cd -- "$prefix" && pwd -P)
[[ $prefix != *$'\n'* ]] || die 'installation prefix cannot contain a newline'
cd -- "$root"

build_log=$(mktemp)
trap 'rm -f -- "$build_log"' EXIT
build_args=(--locked --manifest-path "$root/Cargo.toml" -p vtr-capi --lib --target-dir "$target_dir")
if [[ $profile == release ]]; then
    build_args+=(--release)
fi
if $private_heap; then
    build_args+=(--features private-heap)
fi

if ! CARGO_TERM_COLOR=never "${cargo_cmd[@]}" rustc "${build_args[@]}" -- --print native-static-libs >"$build_log" 2>&1; then
    cat "$build_log" >&2
    die 'Cargo could not build vtr-capi'
fi
cat "$build_log" >&2
native_libs=$(sed -E 's/\x1B\[[0-9;]*[[:alpha:]]//g' "$build_log" | sed -n 's/.*native-static-libs: //p' | tail -n 1)
native_file="$target_dir/$profile/vtr-native-static-libs"
if [[ -n $native_libs ]]; then
    printf '%s\n' "$native_libs" >"$native_file"
elif [[ -f $native_file ]]; then
    native_libs=$(cat "$native_file")
fi
[[ -n $native_libs ]] || die 'rustc did not report native static link libraries'

library="$target_dir/$profile/libvtr.a"
[[ -f $library ]] || die "Cargo did not produce $library"
pkgid=$("${cargo_cmd[@]}" pkgid --locked --manifest-path "$root/Cargo.toml" -p vtr-capi) || die 'Cargo could not read the vtr-capi version'
version=${pkgid##*#}
version=${version##*@}
[[ $version != "$pkgid" && -n $version ]] || die "unexpected Cargo package ID: $pkgid"

install -d -- "$prefix/lib" "$prefix/include" "$prefix/lib/pkgconfig"
install -m 644 -- "$library" "$prefix/lib/libvtr.a"
install -m 644 -- "$root/vtr-capi/include/vtr.h" "$root/vtr-capi/include/vtr_log.hpp" \
    "$root/vtr-capi/include/vtr_track.hpp" "$prefix/include/"
if $with_dpi; then
    install -m 644 -- "$root/vtr-capi/include/vtr_trace_dpi.hpp" \
        "$root/vtr-capi/include/vtr_trace.sv" "$prefix/include/"
fi
cat >"$prefix/lib/pkgconfig/vtr.pc" <<EOF
prefix=$prefix
libdir=\${prefix}/lib
includedir=\${prefix}/include

Name: vtr
Description: VTR static C API
Version: $version
Libs: -L\${libdir} -lvtr
Libs.private: $native_libs
Cflags: -I\${includedir}
EOF
printf 'Installed VTR C API to %s (%s profile)\n' "$prefix" "$profile"
