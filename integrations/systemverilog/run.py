#!/usr/bin/env python3
"""Verify the standalone vtr_trace sink with a simulator other than the fork.

Builds the pipeline tracer suite's design (integrations/verilator/pipeline:
demo_core, its tracer and bind file) with an upstream Verilator, which knows
nothing about VTR: vtr_trace.sv is listed like any package, the standalone
sink (vtr_trace_standalone.cpp) supplies the DPI bodies over VPI time, and the
model links libvtr. In one and two thread modes the file named by
+vtr_trace= must hold exactly the transactions the fork records, the declared
clock and the package's warning; without the plusarg no file is written.

    run.py [--verilator verilator] [--out DIR]
"""
import argparse
import importlib.util
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
PIPELINE = ROOT / 'integrations/verilator/pipeline'

spec = importlib.util.spec_from_file_location('pipeline_suite', PIPELINE / 'run.py')
suite = importlib.util.module_from_spec(spec)
spec.loader.exec_module(suite)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--verilator', default='verilator', help='an upstream Verilator (5.020 or newer)')
    parser.add_argument('--out', type=Path, default=ROOT / 'bench/build/systemverilog-standalone')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    cli = ROOT / 'target/release/vtr'
    lib = ROOT / 'target/release/libvtr.a'
    for f in (cli, lib):
        if not f.exists():
            raise SystemExit(f'{f} missing: cargo build --release -p vtr-capi -p vtr-cli')
    version = subprocess.check_output([args.verilator, '--version'], text=True).strip()
    include = ROOT / 'core/vtr-capi/include'
    sources = [include / 'vtr_trace.sv', PIPELINE / 'core.sv', PIPELINE / 'tracer.sv', PIPELINE / 'bind.sv',
               HERE / 'vtr_trace_standalone.cpp', HERE / 'main.cpp']
    for threads in [1, 2]:
        obj = out / f'obj_{threads}'
        with (out / f'build_{threads}.log').open('w') as log:
            subprocess.run([args.verilator, '--cc', '--exe', '--build', '-j', '4', '--timing', '--vpi',
                            '--threads', str(threads), '--top-module', 'tb', '--Mdir', str(obj),
                            '-CFLAGS', f'-I{include}', '-LDFLAGS', f'{lib} -lpthread -ldl -lm',
                            *map(str, sources), '-o', 'sim'],
                           stdout=log, stderr=subprocess.STDOUT, check=True)
        run = out / f'run_{threads}'
        run.mkdir(exist_ok=True)
        (run / 'pipeline.vtr').unlink(missing_ok=True)
        # Without the plusarg the package records nothing and writes no file.
        subprocess.run([str(obj / 'sim')], cwd=run, capture_output=True, timeout=60, check=True)
        assert not (run / 'pipeline.vtr').exists(), 'a file was written without +vtr_trace='
        result = subprocess.run([str(obj / 'sim'), '+vtr_trace=pipeline.vtr'], cwd=run, capture_output=True,
                                text=True, timeout=60)
        (run / 'console.txt').write_text(result.stdout + result.stderr)
        assert result.returncode == 0, result
        trace = run / 'pipeline.vtr'
        txs = suite.transactions(cli, trace)
        got = suite.canonical(txs)
        if got != suite.EXPECTED:
            import difflib
            raise AssertionError(''.join(difflib.unified_diff(suite.EXPECTED.splitlines(True), got.splitlines(True),
                                                              'fork', 'standalone')))
        edges = suite.clock_edges(cli, trace)
        for t in txs:
            times = [t['begin']] + [b for _, _, b, _ in t['stages']]
            if t['status'] != 'open':
                times += [t['end']] + [int(e) for _, _, _, e in t['stages']]
            assert set(times) <= edges, (t, sorted(set(times) - edges))
        log = subprocess.check_output([str(cli), 'log', str(trace)], text=True)
        warns = [line.split('simulation_log: ', 1)[1] for line in log.splitlines() if ' warn ' in line]
        assert warns == [suite.WARNING], warns
        assert f'%Warning-VTRTRACE: {suite.WARNING}' in result.stdout
        print(f'{version}, {threads} threads: {len(txs)} transactions identical to the fork\'s; '
              f'clock, stage edges, warning and plusarg verified')


if __name__ == '__main__':
    main()
