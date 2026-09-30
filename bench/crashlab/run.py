#!/usr/bin/env python3
"""Crash lab: what reaches a VTR file when the simulation dies, with and without a crash guard.

Builds libvtr twice (the system allocator, and VTR on a private heap via
private-heap.patch applied to a temporary worktree of HEAD), builds the
crashlab prototype and the check reader against each, runs every scenario,
checks each outcome against what docs/crash-safe-vtr.html claims, and writes
bench/results/crashlab.json. Exits nonzero when an outcome differs.

  python3 bench/crashlab/run.py            # full matrix, ~2 minutes
  python3 bench/crashlab/run.py --quick    # fewer repetitions
"""
import argparse
import json
import os
import re
import resource
import shutil
import signal
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
HERE = os.path.join(ROOT, "bench", "crashlab")
BUILD = os.path.join(ROOT, "bench", "build", "crashlab")
INC = os.path.join(ROOT, "core", "vtr-capi", "include")
OUT = os.path.join(ROOT, "bench", "results", "crashlab.json")

SIG = {name: int(getattr(signal, name)) for name in ("SIGSEGV", "SIGABRT", "SIGTERM", "SIGKILL")}


def sh(cmd, **kw):
    print("+", " ".join(cmd) if isinstance(cmd, list) else cmd, flush=True)
    subprocess.run(cmd, check=True, **kw)


def build_libs():
    os.makedirs(BUILD, exist_ok=True)
    sh(["cargo", "build", "--release", "-p", "vtr-capi"], cwd=ROOT)
    stock = os.path.join(ROOT, "target", "release", "libvtr.a")
    wt = os.path.join(BUILD, "wt")
    if os.path.exists(wt):
        subprocess.run(["git", "worktree", "remove", "--force", wt], cwd=ROOT)
        shutil.rmtree(wt, ignore_errors=True)
    sh(["git", "worktree", "add", "--detach", wt, "HEAD"], cwd=ROOT)
    try:
        # Path dependencies into submodules resolve through the main checkout.
        for name in os.listdir(os.path.join(ROOT, "ext")):
            dst = os.path.join(wt, "ext", name)
            if os.path.isdir(dst) and not os.listdir(dst):
                os.rmdir(dst)
                os.symlink(os.path.join(ROOT, "ext", name), dst)
        sh(["git", "apply", os.path.join(HERE, "private-heap.patch")], cwd=wt)
        env = dict(os.environ, CARGO_TARGET_DIR=os.path.join(BUILD, "target"))
        sh(["cargo", "build", "--release", "-p", "vtr-capi"], cwd=wt, env=env)
    finally:
        subprocess.run(["git", "worktree", "remove", "--force", wt], cwd=ROOT)
    private = os.path.join(BUILD, "target", "release", "libvtr.a")
    cxx = os.environ.get("CXX", "clang++" if shutil.which("clang++") else "g++")
    bins = {}
    for tag, lib in (("system", stock), ("private", private)):
        for prog in ("crashlab", "check"):
            exe = os.path.join(BUILD, f"{prog}-{tag}")
            sh([cxx, "-O2", "-g", "-std=c++17", f"-I{INC}", os.path.join(HERE, f"{prog}.cpp"), lib, "-lpthread", "-ldl", "-lm", "-o", exe])
            bins[(prog, tag)] = exe
    return bins


def no_core():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


def run(bins, tag, mode, *args, cpus="0-7"):
    out = os.path.join(BUILD, f"out-{tag}-{mode}.vtr")
    cmd = [bins[("crashlab", tag)], mode, out, *args]
    if shutil.which("taskset"):
        cmd = ["taskset", "-c", cpus, *cmd]
    t0 = time.monotonic()
    p = subprocess.run(cmd, stderr=subprocess.PIPE, text=True, timeout=120, preexec_fn=no_core)
    wall = time.monotonic() - t0
    err = p.stderr
    m = re.findall(r"emitted records=(\d+) logs=(\d+) last_time=(\d+) write_s=([0-9.]+)", err)[-1]
    rescue = re.search(r"rescue_ms=([0-9.]+)", err)
    close = re.search(r"(?:close|stop_close)_ms=([0-9.]+)", err)
    chk = subprocess.run([bins[("check", tag)], out], capture_output=True, text=True, timeout=120)
    got = json.loads(chk.stdout)
    os.remove(out)
    return {
        "status": p.returncode,
        "signal": -p.returncode if p.returncode < 0 else 0,
        "emitted_changes": int(m[0]),
        "emitted_logs": int(m[1]),
        "write_s": float(m[3]),
        "rescue_ms": float(rescue.group(1)) if rescue else None,
        "close_ms": float(close.group(1)) if close else None,
        "timed_out": "rescue timed out" in err,
        "wall_s": round(wall, 2),
        "file": got,
    }


# (id, label, mode, extra args, configs, expectation per config)
# Expectations: "full" = every emitted change and log record plus the crash record, a
# complete trailer; "blocks" = recovered by scanning, only completed blocks survive.
SCENARIOS = [
    ("segv", "SIGSEGV in model code", "segv", [], {"none": "blocks", "system": "full", "private": "full"}, "SIGSEGV"),
    ("abort", "abort() from an assertion", "abort", [], {"none": "blocks", "system": "full", "private": "full"}, "SIGABRT"),
    ("throw", "Uncaught C++ exception", "throw", [], {"none": "blocks", "system": "full", "private": "full"}, "SIGABRT"),
    ("stack", "Stack overflow", "stack", [], {"none": "blocks", "system": "full", "private": "full"}, "SIGSEGV"),
    ("heap", "Double free, tcache", "heap", [], {"none": "blocks", "system": "full", "private": "full"}, "SIGABRT"),
    ("heaplock", "Double free, arena lock held", "heaplock", ["--timeout-ms", "3000"], {"none": "blocks", "system": "blocks", "private": "full"}, "SIGABRT"),
    ("term", "SIGTERM from a job scheduler", "term", [], {"none": "blocks", "system": "full", "private": "full"}, "SIGTERM"),
    ("exit", "exit(3) from C code", "exit", [], {"none": "blocks", "system": "full", "private": "full"}, None),
    ("kill", "SIGKILL or the OOM killer", "kill", [], {"none": "blocks", "system": "blocks", "private": "blocks"}, "SIGKILL"),
    ("stall", "Rescue stuck past the deadline", "segv", ["--stall-ms", "5000", "--timeout-ms", "1000"], {"system": "blocks", "private": "blocks"}, "SIGSEGV"),
    ("noalt", "Stack overflow, no alternate stack", "stack", ["--no-altstack"], {"system": "blocks", "private": "blocks"}, "SIGSEGV"),
]


def verdict(r, expect, sig_name):
    f = r["file"]
    problems = []
    if sig_name:
        if r["signal"] != SIG[sig_name]:
            problems.append(f"expected death by {sig_name}, got status {r['status']}")
    elif r["status"] != 3:
        problems.append(f"expected exit status 3, got {r['status']}")
    if expect == "full":
        if f["recovered"] or f["changes"] != r["emitted_changes"] or f["logs"] != r["emitted_logs"] + 1 or not f["fatal"]:
            problems.append(f"expected everything kept, got {f}")
    else:
        if not f["recovered"] or f["changes"] >= r["emitted_changes"] or f["logs"] != 0:
            problems.append(f"expected a recovered file with completed blocks only, got {f}")
    return problems


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--quick", action="store_true")
    a = ap.parse_args()
    reps = 1 if a.quick else 3
    bins = build_libs()
    results = {"host": {}, "scenarios": [], "drain": [], "busy": []}
    try:
        results["host"]["cpu"] = next(l.split(":", 1)[1].strip() for l in open("/proc/cpuinfo") if l.startswith("model name"))
    except (OSError, StopIteration):
        pass
    results["host"]["glibc"] = os.confstr("CS_GNU_LIBC_VERSION")
    failures = []
    for sid, label, mode, extra, configs, sig_name in SCENARIOS:
        row = {"id": sid, "label": label, "results": {}}
        for cfg, expect in configs.items():
            tag = "system" if cfg == "none" else cfg
            args = [*extra, "--no-guard"] if cfg == "none" else extra
            r = run(bins, tag, mode, *args)
            r["expect"] = expect
            problems = verdict(r, expect, sig_name)
            r["ok"] = not problems
            row["results"][cfg] = r
            kept = r["file"]["changes"] / r["emitted_changes"]
            print(f"{sid:9} {cfg:8} status={r['status']:4} kept={kept:6.1%} logs={r['file']['logs']}/{r['emitted_logs']} rescue_ms={r['rescue_ms']} {'ok' if not problems else problems}", flush=True)
            failures += [f"{sid}/{cfg}: {p}" for p in problems]
        results["scenarios"].append(row)
    # Drain: how long the rescue takes, by where in a block the crash lands (16 Mi changes per block).
    for records, where in ((17000000, "just after a block was handed off"), (30000000, "a block 79% full"), (33540000, "a block 99.9% full")):
        times = [run(bins, "private", "segv", "--records", str(records))["rescue_ms"] for _ in range(reps)]
        results["drain"].append({"records": records, "where": where, "rescue_ms": min(times), "runs": times})
        print(f"drain {records}: {min(times)} ms", flush=True)
    # Busy flag: the cost of telling the rescue whether the owner is inside the writer.
    for m in ("", "--busy-step", "--busy-call"):
        times = [run(bins, "private", "none", "--records", "60000000", *([m] if m else []))["write_s"] for _ in range(reps + 2)]
        results["busy"].append({"mode": m or "off", "write_s": min(times), "runs": times})
        print(f"busy {m or 'off'}: {min(times)} s", flush=True)
    results["failures"] = failures
    with open(OUT, "w") as f:
        json.dump(results, f, indent=1)
    print(f"wrote {OUT}")
    if failures:
        print("FAILED:\n  " + "\n  ".join(failures))
        sys.exit(1)


if __name__ == "__main__":
    main()
