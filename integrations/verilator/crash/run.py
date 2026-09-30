#!/usr/bin/env python3
"""A --trace-vtr model that dies in every catchable way keeps its trace.

Builds integrations/verilator/crash with the fork's Verilator (one and two
threads), runs it to completion as the reference, then once per ending: a DPI
function that segfaults, aborts, recurses without bound or calls exit(3) at a
given cycle, SIGTERM while it runs, and $fatal from the design. Each file must
be complete (not recovered), record how the run ended, and hold the same value
changes and simulation log records as the finished run up to the cycle before
the crash; the process must end with the same status as without the guard.
"""
import argparse
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
CRASH = 5000  # cycle; it happens at time 2 * CRASH + 1, during eval, before that dump

# mode -> (exit status as returncode, the ending `vtr info` prints)
ENDINGS = {
    "segv": (-11, "crashed by SIGSEGV"),
    "abort": (-6, "crashed by SIGABRT"),
    "stack": (-11, "crashed by SIGSEGV"),
    "exit": (3, "exited with status 3"),
    "term": (-15, "stopped by SIGTERM"),
}


def vcd_changes(tool, trace, out, until):
    """Value changes at times <= until, as (time, line) pairs."""
    subprocess.run([str(tool), str(trace), str(out)], check=True, capture_output=True)
    changes, t, body = [], 0, False
    for line in out.read_text().splitlines():
        if line.startswith("$enddefinitions"):
            body = True
        elif body and line.startswith("#"):
            t = int(line[1:])
        elif body and line and not line.startswith("$") and t <= until:
            changes.append((t, line))
    return changes


def log_records(cli, trace, until):
    text = subprocess.check_output([str(cli), "log", str(trace)], text=True)
    rows = re.findall(r"(?m)^(\d+) (\w+) +([\w.]+): (.*)$", text)
    return [r for r in rows if r[2] == "simulation_log" and int(r[0]) <= until]


def ending(cli, trace):
    info = subprocess.check_output([str(cli), "info", str(trace)], text=True)
    version = re.search(r"(?m)^version: .*$", info)[0]
    return re.search(r"(?m)^ended: +(.*)$", info)[1], "recovered" in version


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verilator", type=Path, required=True)
    parser.add_argument("--out", type=Path, default=ROOT / "bench/build/verilator-crash")
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, VTR_INCLUDE=str(ROOT / "core/vtr-capi/include"), VTR_LIBDIR=str(ROOT / "target/release"))
    cli, vcd = ROOT / "target/release/vtr", ROOT / "target/release/vtr2vcd"
    until = 2 * CRASH
    failures = []
    for threads in [1, 2]:
        obj = out / f"obj_{threads}"
        with (out / f"build_{threads}.log").open("w") as log:
            subprocess.run([str(args.verilator.resolve()), "--cc", "--exe", "--build", "-j", "4", "--trace-vtr",
                            "--threads", str(threads), "--top-module", "top", "--Mdir", str(obj),
                            str(HERE / "top.sv"), str(HERE / "main.cpp"), "-o", "sim"],
                           env=env, stdout=log, stderr=subprocess.STDOUT, check=True)

        def sim(name, *plusargs):
            run = out / f"{name}_{threads}"
            run.mkdir(exist_ok=True)
            r = subprocess.run([str(obj / "sim"), *plusargs], cwd=run, capture_output=True, text=True, errors="replace", timeout=60,
                               env=dict(env, LD_LIBRARY_PATH=env["VTR_LIBDIR"]))
            (run / "console.txt").write_text(r.stdout + r.stderr)
            return r, run / "crash.vtr"

        r, ref = sim("finished")
        assert r.returncode == 0, r
        assert ending(cli, ref) == ("closed", False), ending(cli, ref)
        want_changes = vcd_changes(vcd, ref, out / "ref.vcd", until)
        want_logs = log_records(cli, ref, until)
        assert len(want_logs) > 50 and len(want_changes) > 10000, (len(want_logs), len(want_changes))
        # Crash safety is opt-in: without the guard the file keeps only what the encoder wrote.
        r, trace = sim("noguard", "+mode=segv", f"+crash={CRASH}", "+noguard")
        got_end, recovered = ending(cli, trace)
        if r.returncode != -11 or not recovered:
            failures.append(f"unguarded segv, {threads} thread(s): status {r.returncode}, ended {got_end!r}, recovered {recovered}")
        print(f"unguarded segv, {threads} thread(s): status {r.returncode}, ended {got_end!r}", flush=True)
        cases = [(m, (f"+mode={m}", f"+crash={CRASH}")) for m in ENDINGS] + [("fatal", (f"+fatal={CRASH}",))]
        for mode, plusargs in cases:
            r, trace = sim(mode, *plusargs)
            got_end, recovered = ending(cli, trace)
            problems = []
            if mode == "fatal":
                # Verilator's own exit callbacks close the trace, then the process aborts.
                want_status, want_end = r.returncode, "closed"
                if r.returncode == 0:
                    problems.append("$fatal ended with status 0")
            else:
                want_status, want_end = ENDINGS[mode]
            if r.returncode != want_status:
                problems.append(f"status {r.returncode}, expected {want_status}")
            if recovered or not got_end.startswith(want_end):
                problems.append(f"ended {got_end!r} (recovered: {recovered}), expected {want_end!r}")
            got_changes = vcd_changes(vcd, trace, out / "got.vcd", until)
            if got_changes != want_changes:
                problems.append(f"{len(got_changes)} value changes up to t={until}, the finished run has {len(want_changes)}")
            got_logs = log_records(cli, trace, until)
            if got_logs != want_logs:
                problems.append(f"{len(got_logs)} log records up to t={until}, the finished run has {len(want_logs)}")
            label = f"{mode}, {threads} thread(s)"
            if problems:
                failures.append(f"{label}: {'; '.join(problems)}\n{r.stdout}{r.stderr}")
            print(f"{label}: status {r.returncode}, ended {got_end!r}, {len(got_changes)} changes and {len(got_logs)} records up to t={until} "
                  f"{'match' if not problems else 'DIFFER'}", flush=True)
    if failures:
        raise SystemExit("FAILED:\n" + "\n".join(failures))


if __name__ == "__main__":
    main()
