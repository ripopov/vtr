#!/usr/bin/env python3
"""Alternate baseline/current local or remote loads; retain every process sample."""
import argparse
import json
import resource
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("current", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--cpus", default="0-7")
    parser.add_argument("--remote-servers", nargs=2, type=Path, metavar=("BASELINE", "CURRENT"))
    parser.add_argument("--transaction-trace", type=Path, default=Path("bench/results/latest/tlm_1m.vtr"))
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("--samples must be positive")
    root = Path(__file__).resolve().parents[3]
    binaries = {"baseline": args.baseline.resolve(), "current": args.current.resolve()}
    workloads = [
        ("volna/volna/examples/picorv32.vtr", "64"),
        ("bench/results/latest/rsa256.vtr", "64"),
        ("bench/workloads/gen/rsa256.fst", "64"),
        (str(args.transaction_trace), "tracks"),
    ]
    servers = None
    if args.remote_servers:
        servers = dict(zip(("baseline", "current"), (p.resolve() for p in args.remote_servers)))
        workloads = [(path, f"signals:{selection}" if selection.isdigit() else selection)
                     for path, selection in workloads]
        workloads.append(("bench/results/latest/kanata_sample2.vtr", "tracks"))
    for path, _ in workloads:
        if not (root / path).is_file():
            parser.error(f"missing benchmark fixture: {root / path}")
    with args.output.open("w") as output:
        for path, selection in workloads:
            counts = None
            for sample in range(args.samples + 1):
                variants = ["baseline", "current"] if sample % 2 == 0 else ["current", "baseline"]
                for variant in variants:
                    command = ["taskset", "-c", args.cpus, str(binaries[variant])]
                    if servers:
                        command.append(str(servers[variant]))
                    command.extend((str(root / path), selection))
                    before = resource.getrusage(resource.RUSAGE_CHILDREN)
                    result = subprocess.run(
                        command,
                        capture_output=True, text=True, check=True, timeout=120,
                    )
                    after = resource.getrusage(resource.RUSAGE_CHILDREN)
                    row = json.loads(result.stdout)
                    row["process_cpu_ms"] = 1000 * (after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime)
                    observed = tuple(row.get(key) for key in ("signals", "changes", "transactions", "incident_relation_copies", "errors"))
                    if counts is None:
                        counts = observed
                    if counts != observed:
                        raise RuntimeError(f"different loaded record counts: {path}")
                    row.update(path=path, variant=variant, sample=sample, warmup=sample == 0)
                    output.write(json.dumps(row) + "\n")
                    output.flush()
            print(path, "complete", flush=True)


if __name__ == "__main__":
    main()
