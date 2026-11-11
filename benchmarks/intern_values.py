#!/usr/bin/env python3
"""Compare complete compressed traces with inline and interned instruction descriptions."""

import argparse
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--count", type=int, default=250_000)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--report", type=Path, default=HERE / "intern-values.md")
    args = parser.parse_args()
    if args.count < 1 or args.repeats < 1:
        parser.error("count and repeats must be positive")
    cpu = min(os.sched_getaffinity(0))
    os.sched_setaffinity(0, {cpu})
    binary = HERE / ".build" / "intern_values"
    rows = []
    with tempfile.TemporaryDirectory(prefix="vtr-intern-") as directory:
        for repeat in range(args.repeats):
            for codec in ("lz4", "zstd"):
                variants = ("inline", "interned") if repeat % 2 == 0 else ("interned", "inline")
                for variant in variants:
                    path = Path(directory) / f"{codec}-{variant}.vtr"
                    for operation in ("write", "read"):
                        result = json.loads(subprocess.check_output(
                            [str(binary), str(path), variant, codec, str(args.count), operation],
                            text=True))
                        rows.append(dict(codec=codec, variant=variant, operation=operation,
                                         repeat=repeat, **result))
                        print(f"verified {codec} {variant} {operation} repeat {repeat + 1}", flush=True)
    for codec in ("lz4", "zstd"):
        checksums = {r["checksum"] for r in rows if r["codec"] == codec and r["operation"] == "read"}
        assert len(checksums) == 1, "decoded contents differ"
        sizes = {variant: {r["file_bytes"] for r in rows if r["codec"] == codec and r["variant"] == variant}
                 for variant in ("inline", "interned")}
        assert all(len(values) == 1 for values in sizes.values()), "file size changed across repeats"
        assert next(iter(sizes["interned"])) < next(iter(sizes["inline"])), "interning did not reduce file size"
    lines = ["# Interned instruction values", "",
             f"Host: {platform.platform()}. CPU affinity: {cpu}. Repeats: {args.repeats}. "
             f"Transactions per trace: {args.count:,}.", "",
             "Each trace replays the same interleaved sequence of 1,024 instruction descriptions. "
             "Each description contains an instruction pointer, 32 machine-code bytes, a mnemonic, and an execution pipe. "
             "A dynamic execution mask stays inline. Both variants use the same API, compression, and buffer settings. "
             "Write time includes table construction, capture, encoding, compression, and close. "
             "Read time includes opening, decoding, resolving, and independently checking every instruction and mask. "
             "File size includes all declaration tables, metadata, section headers, and indexes. "
             "Fresh processes measure peak RSS; medians report time and the maximum reports RSS. "
             "Variant order alternates across repeats. OS page cache is warm after writing.", "",
             "| Codec | Representation | Bytes/transaction | File bytes | Write (ms) | Read (ms) | Write RSS (MiB) | Read RSS (MiB) |",
             "|---|---|---:|---:|---:|---:|---:|---:|"]
    for codec in ("lz4", "zstd"):
        for variant in ("inline", "interned"):
            selected = [r for r in rows if r["codec"] == codec and r["variant"] == variant]
            size = selected[0]["file_bytes"]
            def med(operation):
                return statistics.median(r["seconds"] for r in selected if r["operation"] == operation) * 1000
            def rss(operation):
                return max(r["peak_rss_kib"] for r in selected if r["operation"] == operation) / 1024
            lines.append(f"| {codec.upper()} | {variant} | {size / args.count:.3f} | {size:,} | "
                         f"{med('write'):.2f} | {med('read'):.2f} | {rss('write'):.2f} | {rss('read'):.2f} |")
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text("\n".join(lines) + "\n")
    args.report.with_suffix(".json").write_text(json.dumps(dict(host=platform.platform(), cpu=cpu,
        count=args.count, repeats=args.repeats, build=json.loads((HERE / ".build/build.json").read_text()),
        measurements=rows), indent=2) + "\n")


if __name__ == "__main__":
    main()
