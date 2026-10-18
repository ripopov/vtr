#!/usr/bin/env python3
"""Build the C++ benchmark with pinned upstream FST/FTR implementations."""

import argparse
import json
import os
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
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

    cargo = ["cargo", "build", "--locked", "--release", "-p", "vtr-capi"]
    if args.offline:
        cargo.append("--offline")
    run(cargo, cwd=ROOT)
    metadata = json.loads(output(["cargo", "metadata", "--no-deps", "--format-version=1", "--offline"], cwd=ROOT))
    libvtr = Path(metadata["target_directory"]) / "release" / "libvtr.a"
    cc, cxx = os.environ.get("CC", "cc"), os.environ.get("CXX", "c++")
    fst = BUILD / "deps" / "libfst" / "src"
    objects = []
    for source in ("fstapi.c", "fastlz.c", "lz4.c"):
        obj = BUILD / (source + ".o")
        run([cc, "-O3", "-DNDEBUG", "-D_GNU_SOURCE", "-DHAVE_FSEEKO", "-DHAVE_REALPATH",
             "-I", fst, "-c", fst / source, "-o", obj])
        objects.append(obj)
    run([cxx, "-std=c++17", "-O3", "-DNDEBUG", "-Wall", "-Wextra", "-Wpedantic",
         "-I", ROOT / "vtr-capi" / "include", "-isystem", fst,
         "-isystem", BUILD / "deps" / "lwtr" / "src", HERE / "benchmark.cpp", *objects,
         libvtr, "-lz", "-lbz2", "-ldl", "-lpthread", "-lm", "-o", BUILD / "benchmark"])
    (BUILD / "build.json").write_text(json.dumps({
        "dependencies": {name: {"url": url, "revision": revision} for name, (url, revision) in DEPS.items()},
        "vtr_revision": output(["git", "rev-parse", "HEAD"], cwd=ROOT),
        "vtr_dirty": bool(output(["git", "status", "--porcelain", "--untracked-files=no"], cwd=ROOT)),
        "compiler": output([cxx, "--version"]).splitlines()[0],
        "rustc": output(["rustc", "--version"], cwd=ROOT),
        "flags": "C++17 -O3 -DNDEBUG; VTR release; synchronous LZ4 writers",
        "lz4": "FST and FTR: libfst bundled LZ4; VTR: Cargo.lock lz4_flex",
    }, indent=2) + "\n")
    print(f"Built {BUILD / 'benchmark'}")


if __name__ == "__main__":
    main()
