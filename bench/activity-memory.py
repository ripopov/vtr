#!/usr/bin/env python3
"""Stage 7 paging gate: retained index bytes and fresh-process peak RSS (Linux).

Build vtr-bench with cargo build --release -p vtr-bench, then pass VTR/FST
traces. Each trace gets a new sidecar in an isolated temporary directory;
every load sample runs in a new child with no source trace resident.
"""
import argparse
import json
import platform
import subprocess
import tempfile
from pathlib import Path


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('traces', type=Path, nargs='*')
    p.add_argument('--cohort', type=Path, help='reuse the trace paths from a previous report')
    p.add_argument('--binary', type=Path, default=Path('target/release/vtr-bench'))
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--samples', type=int, default=3)
    p.add_argument('--threads', type=int, default=8)
    p.add_argument('--cpus', default='0-7', help='empty for an unpinned CI smoke check')
    p.add_argument('--object-mib', type=int, default=256)
    p.add_argument('--budget-mib', type=int, default=512)
    a = p.parse_args()
    if platform.system() != 'Linux':
        p.error('Linux /proc peak RSS accounting is required')
    if min(a.samples, a.threads, a.object_mib, a.budget_mib) <= 0:
        p.error('samples, threads and limits must be positive')
    if bool(a.traces) == bool(a.cohort):
        p.error('provide trace paths or --cohort, exclusively')
    binary = a.binary.resolve(strict=True)
    inputs = a.traces if a.traces else [Path(r['trace']) for r in json.loads(a.cohort.read_text())['runs']]
    if not inputs:
        p.error('the cohort must contain traces')
    traces = [t.resolve(strict=True) for t in inputs]
    prefix = ['taskset', '-c', a.cpus] if a.cpus else []

    def run(*args):
        result = subprocess.run([*prefix, str(binary), *map(str, args)],
                                capture_output=True, text=True, check=True, timeout=300)
        return json.loads(result.stdout)

    report = {
        'machine': {'cpu': next(line.split(':', 1)[1].strip()
                               for line in Path('/proc/cpuinfo').read_text().splitlines()
                               if line.startswith('model name')),
                    'kernel': platform.release(),
                    'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip()},
        'cpus': a.cpus, 'samples_per_trace': a.samples, 'threads': a.threads,
        'object_limit_bytes': a.object_mib << 20, 'budget_bytes': a.budget_mib << 20,
        'runs': [],
    }
    with tempfile.TemporaryDirectory(prefix='vtr-activity-memory-') as scratch:
        for k, trace in enumerate(traces):
            index = Path(scratch) / f'{k}.index'
            build = run('activity', trace, '--threads', a.threads, '--output', index)
            samples = [run('activity-load', index, '--format', build['format'],
                           '--length', build['file_bytes'], '--toc-crc', build['toc_crc'])
                       for _ in range(a.samples)]
            expected = (build['loaded_bytes'], build['signals'], build['blocks'], build['stretches'])
            for sample in samples:
                actual = tuple(sample[key] for key in ['loaded_bytes', 'signals', 'blocks', 'stretches'])
                if actual != expected or sample['peak_rss_bytes'] <= 0 or sample['load_ms'] <= 0:
                    raise RuntimeError(f'invalid sidecar load measurement: {sample}')
            peak = max(s['peak_rss_bytes'] for s in samples)
            needs_paging = build['loaded_bytes'] > report['object_limit_bytes'] or peak > report['budget_bytes']
            report['runs'].append({
                'trace': str(trace.relative_to(Path.cwd())) if trace.is_relative_to(Path.cwd()) else str(trace),
                'trace_bytes': build['file_bytes'], 'format': build['format'],
                'signals': build['signals'], 'blocks': build['blocks'],
                'stretches': build['stretches'], 'sidecar_bytes': build['sidecar_bytes'],
                'loaded_bytes': build['loaded_bytes'], 'peak_rss_bytes': peak,
                'load_ms': min(s['load_ms'] for s in samples),
                'needs_paging': needs_paging, 'samples': samples,
            })
            index.unlink()
            print(f'{trace.name}: loaded {build["loaded_bytes"] / (1 << 20):.2f} MiB, '
                  f'peak {peak / (1 << 20):.2f} MiB, paging {needs_paging}', flush=True)
    report['needs_paging'] = any(r['needs_paging'] for r in report['runs'])
    a.output.parent.mkdir(parents=True, exist_ok=True)
    a.output.write_text(json.dumps(report, indent=2) + '\n')
    # A measured limit breach must fail CI rather than silently approve the gate.
    if report['needs_paging']:
        raise SystemExit('activity index exceeds the configured paging gate; see the report')


if __name__ == '__main__':
    main()
