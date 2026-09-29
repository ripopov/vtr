#!/usr/bin/env python3
"""Fresh-process hierarchy open measurements, pinned best-of-N (Linux).

python3 bench/hierarchy-cost.py --bin-dir DIR --fixtures DIR --output FILE
Build load_cost, remote_cost and volna-server with cargo build --release first.
Fixtures: vtr-bench gen-gates c910_coremark.vtr gatesN.vtr --copies N;
FST twins: vtr to-vcd gatesN.vtr gatesN.vcd; vcd2fst gatesN.vcd gatesN.fst.
Use --baseline-bin-dir and --baseline-output for interleaved A/B samples.
"""
import argparse
import json
import platform
import subprocess
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--bin-dir', type=Path, required=True)
p.add_argument('--fixtures', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--baseline-bin-dir', type=Path)
p.add_argument('--baseline-output', type=Path)
p.add_argument('--samples', type=int, default=3)
p.add_argument('--cpus', default='0-7')
a = p.parse_args()
if a.samples <= 0 or bool(a.baseline_bin_dir) != bool(a.baseline_output):
    p.error('positive samples and both baseline arguments are required')
machine = {
    'cpu': next(line.split(':', 1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')),
    'kernel': platform.release(),
    'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(),
}
configs = [('after', a.bin_dir, a.output)]
if a.baseline_bin_dir:
    configs.insert(0, ('before', a.baseline_bin_dir, a.baseline_output))
rows = {tag: [] for tag, _, _ in configs}
for name in ['gates1.vtr', 'gates4.vtr', 'gates16.vtr', 'gates1.fst', 'gates4.fst']:
    trace = a.fixtures / name
    if not trace.is_file():
        raise SystemExit(f'missing generated fixture: {trace}')
    modes = ['local'] if name == 'gates16.vtr' else ['local', 'remote']
    for mode in modes:
        samples = {tag: [] for tag, _, _ in configs}
        for _ in range(a.samples):
            for tag, binaries, _output in configs:
                cmd = ([str(binaries / 'examples/load_cost'), str(trace), '0'] if mode == 'local' else
                       [str(binaries / 'examples/remote_cost'), str(binaries / 'volna-server'), str(trace), 'signals:0', '16384', '16384'])
                result = subprocess.run(['taskset', '-c', a.cpus, *cmd], capture_output=True, text=True, check=True, timeout=300)
                sample = json.loads(result.stdout)
                if sample.get('errors') or sample['open_ms'] <= 0:
                    raise RuntimeError(f'failed measurement: {sample}')
                samples[tag].append(sample)
        for tag, _, output in configs:
            # Preserve all runs; select one complete fastest-open row.
            best = min(samples[tag], key=lambda s: s['open_ms'])
            rows[tag].append({'trace': name, 'trace_bytes': trace.stat().st_size, 'mode': mode,
                              'best': best, 'samples': samples[tag]})
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(json.dumps({'cpus': a.cpus, 'samples_per_workload': a.samples,
                                          'interleaved_baseline': bool(a.baseline_bin_dir),
                                          'machine': machine, 'runs': rows[tag]}, indent=2) + '\n')
            print(tag, name, mode, best['open_ms'], flush=True)
