#!/usr/bin/env python3
"""Build the C++ benchmark with pinned upstream FST/FTR implementations."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
BUILD = HERE / ".build"
DEPS = {
    "libfst": ("https://github.com/gtkwave/libfst.git", "17f6a5d27b2f576d65b550114fbde3c035cc74a8"),
    "lwtr": ("https://github.com/Minres/LWTR4SC.git", "8dca86925ad64b0ca73825fcefd7308215049627"),
}


def run(args, **kwargs):
    subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def output(args, **kwargs):
    return subprocess.check_output([str(arg) for arg in args], text=True, **kwargs).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true", help="require already fetched dependency revisions")
    args = parser.parse_args()
    containers = ROOT / "vtr-cpp" / "third_party" / "unordered_dense"
    container_manifest = json.loads((containers / "UPSTREAM.json").read_text())
    for path, expected in container_manifest["files"].items():
        if hashlib.sha256((containers / path).read_bytes()).hexdigest() != expected:
            parser.error(f"vendored container checksum mismatch: {containers / path}")
    BUILD.mkdir(exist_ok=True)
    for name, (url, revision) in DEPS.items():
        dest = BUILD / "deps" / name
        if not (dest / ".git").exists():
            if args.offline:
                parser.error(f"missing {name}; run build.py once with network access")
            dest.mkdir(parents=True, exist_ok=True)
            run(["git", "init", dest])
            run(["git", "-C", dest, "remote", "add", "origin", url])
        available = subprocess.run(["git", "-C", str(dest), "cat-file", "-e", revision], capture_output=True).returncode == 0
        if not available:
            if args.offline:
                parser.error(f"missing revision {revision} for {name}")
            run(["git", "-C", dest, "fetch", "--depth=1", "origin", revision])
        if output(["git", "-C", dest, "status", "--porcelain"]):
            parser.error(f"dependency checkout has local changes: {dest}")
        run(["git", "-C", dest, "checkout", "--detach", revision])

    cc, cxx = os.environ.get("CC", "cc"), os.environ.get("CXX", "c++")
    native = BUILD / "vtr-cpp"
    run(["cmake", "-S", ROOT / "vtr-cpp", "-B", native, "-DCMAKE_BUILD_TYPE=Release",
         "-DBUILD_TESTING=OFF", f"-DCMAKE_CXX_COMPILER={cxx}"])
    run(["cmake", "--build", native, "--parallel", "2"])
    libvtr = native / "libvtr.a"
    fst = BUILD / "deps" / "libfst" / "src"
    objects = []
    for source in ("fstapi.c", "fastlz.c", "lz4.c"):
        obj = BUILD / (source + ".o")
        run([cc, "-O3", "-DNDEBUG", "-D_GNU_SOURCE", "-DHAVE_FSEEKO", "-DHAVE_REALPATH",
             "-I", fst, "-c", fst / source, "-o", obj])
        objects.append(obj)
    run([cxx, "-std=c++20", "-O3", "-DNDEBUG", "-Wall", "-Wextra", "-Wpedantic",
         "-I", ROOT / "vtr-cpp", "-isystem", fst,
         "-isystem", BUILD / "deps" / "lwtr" / "src", HERE / "benchmark.cpp", *objects,
         libvtr, "-llz4", "-lzstd", "-lz", "-lbz2", "-ldl", "-lpthread", "-lm", "-o", BUILD / "benchmark"])
    (BUILD / "build.json").write_text(json.dumps({
        "dependencies": {name: {"url": url, "revision": revision} for name, (url, revision) in DEPS.items()},
        "vtr_revision": output(["git", "rev-parse", "HEAD"], cwd=ROOT),
        "vtr_dirty": bool(output(["git", "status", "--porcelain", "--untracked-files=no"], cwd=ROOT)),
        "compiler": output([cxx, "--version"]).splitlines()[0],
        "vtr_implementation": "native C++20",
        "vtr_containers": container_manifest,
        "flags": "C++20 -O3 -DNDEBUG; native VTR release; synchronous LZ4 writers",
        "lz4": "FST and FTR: libfst bundled LZ4; VTR: system liblz4",
    }, indent=2) + "\n")
    print(f"Built {BUILD / 'benchmark'}")


if __name__ == "__main__":
    main()
