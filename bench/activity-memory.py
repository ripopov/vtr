#!/usr/bin/env python3
"""Stage 7 paging gate: retained index bytes and fresh-process peak RSS (Linux).

Build vtr-bench with cargo build --release -p vtr-bench, then pass VTR/FST
traces. Each trace gets a new sidecar in an isolated temporary directory;
every load sample runs in a new child with no source trace resident.
"""
import argparse
import hashlib
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
    p.add_argument('--baseline-binary', type=Path, help='interleave A/B builds and fresh-process loads')
    p.add_argument('--baseline-output', type=Path)
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
    if bool(a.baseline_binary) != bool(a.baseline_output):
        p.error('--baseline-binary and --baseline-output must be supplied together')
    baseline = a.baseline_binary.resolve(strict=True) if a.baseline_binary else None
    inputs = a.traces if a.traces else [Path(r['trace']) for r in json.loads(a.cohort.read_text())['runs']]
    if not inputs:
        p.error('the cohort must contain traces')
    traces = [t.resolve(strict=True) for t in inputs]
    prefix = ['taskset', '-c', a.cpus] if a.cpus else []

    def run(binary, *args):
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
    reports = {'current': report}
    if baseline:
        reports['baseline'] = {**report, 'runs': []}
    with tempfile.TemporaryDirectory(prefix='vtr-activity-memory-') as scratch:
        for k, trace in enumerate(traces):
            builds, loads, hashes = {}, {}, {}
            variants = [('current', binary)] + ([('baseline', baseline)] if baseline else [])
            # A/B mode measures every build and load in the same session,
            # alternating the order to balance file-cache and thermal effects.
            for sample in range(a.samples if baseline else 1):
                order = variants if sample % 2 == 0 else variants[::-1]
                for label, executable in order:
                    index = Path(scratch) / f'{k}-{label}-{sample}.index'
                    build = run(executable, 'activity', trace, '--threads', a.threads, '--output', index)
                    builds.setdefault(label, []).append(build)
                    digest = hashlib.sha256(index.read_bytes()).hexdigest()
                    if label in hashes and hashes[label] != digest:
                        raise RuntimeError(f'nondeterministic sidecar for {trace}: {label}')
                    hashes[label] = digest
                    samples = [run(executable, 'activity-load', index, '--format', build['format'],
                                   '--length', build['file_bytes'], '--toc-crc', build['toc_crc'])
                               for _ in range(1 if baseline else a.samples)]
                    expected = (build['loaded_bytes'], build['signals'], build['blocks'], build['stretches'])
                    for measurement in samples:
                        actual = tuple(measurement[key] for key in ['loaded_bytes', 'signals', 'blocks', 'stretches'])
                        if actual != expected or measurement['peak_rss_bytes'] <= 0 or measurement['load_ms'] <= 0:
                            raise RuntimeError(f'invalid sidecar load measurement: {measurement}')
                    loads.setdefault(label, []).extend(samples)
                    index.unlink()
            if baseline and len(set(hashes.values())) != 1:
                raise RuntimeError(f'A/B sidecar bytes differ for {trace}')
            for label, _ in variants:
                build = builds[label][0]
                samples = loads[label]
                peak = max(s['peak_rss_bytes'] for s in samples)
                needs_paging = build['loaded_bytes'] > report['object_limit_bytes'] or peak > report['budget_bytes']
                reports[label]['runs'].append({
                    'trace': str(trace.relative_to(Path.cwd())) if trace.is_relative_to(Path.cwd()) else str(trace),
                    'trace_bytes': build['file_bytes'], 'format': build['format'],
                    'signals': build['signals'], 'blocks': build['blocks'],
                    'stretches': build['stretches'], 'sidecar_bytes': build['sidecar_bytes'],
                    'sidecar_sha256': hashes[label],
                    'build_s': min(b['build_s'] for b in builds[label]),
                    'build_samples_s': [b['build_s'] for b in builds[label]],
                    'loaded_bytes': build['loaded_bytes'], 'peak_rss_bytes': peak,
                    'load_ms': min(s['load_ms'] for s in samples),
                    'needs_paging': needs_paging, 'samples': samples,
                })
                print(f'{label} {trace.name}: loaded {build["loaded_bytes"] / (1 << 20):.2f} MiB, '
                      f'peak {peak / (1 << 20):.2f} MiB, paging {needs_paging}', flush=True)
    for label, path in [('current', a.output)] + ([('baseline', a.baseline_output)] if baseline else []):
        result = reports[label]
        result['needs_paging'] = any(r['needs_paging'] for r in result['runs'])
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(result, indent=2) + '\n')
    # A measured limit breach must fail CI rather than silently approve the gate.
    if any(r['needs_paging'] for r in reports.values()):
        raise SystemExit('activity index exceeds the configured paging gate; see the report')


if __name__ == '__main__':
    main()
