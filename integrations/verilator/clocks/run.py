#!/usr/bin/env python3
"""Build and verify clocks declared through the vtr_trace package (docs/vtr_clocks.html).

Checks, in one and two thread modes: the package resolves with --trace-vtr
alone; the recorded stretches of a DVFS clock (slower and faster) and of a
gated clock reproduce the rising edges of their dumped waveforms; the scope
paths "", "^" and $root; the unit conversion, including a period that is not a
whole number of file units; the report of a run on a running clock; a clock
declared and started before the file opens; a signal-free open; and that a
design naming nothing from the package gets no package code.
"""
import argparse
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent


def clocks(cli, trace):
    """{path: (stretches [(begin, end, period)], running at close)} from `vtr clocks`."""
    out = {}
    current = None
    for line in subprocess.check_output([str(cli), 'clocks', str(trace)], text=True).splitlines():
        m = re.match(r'clock \d+ (\S+): (\d+) edges, (\d+) stretches, stopped \d+(, running at close)?$', line)
        if m:
            current = out.setdefault(m[1], ([], bool(m[4])))
            continue
        m = re.match(r'\s+\[(\d+) \.\. (\d+)\] (?:period (\d+)|single edge)', line)
        assert m and current is not None, line
        current[0].append((int(m[1]), int(m[2]), int(m[3] or 0)))
    return out


def stretch_edges(stretches):
    return [b + k * p for b, e, p in stretches for k in range((e - b) // p + 1 if p else 1)]


def rising_edges(cli, trace, path):
    edges, prev = [], None
    for line in subprocess.check_output([str(cli), 'changes', str(trace), path], text=True).splitlines():
        t, v = line.split('\t')
        if v == '1' and prev == '0':
            edges.append(int(t))
        prev = v
    return edges


def warnings(cli, trace):
    text = subprocess.check_output([str(cli), 'log', str(trace)], text=True)
    return [line for line in text.splitlines() if re.match(r'\d+ warn +simulation_log: vtr_trace: ', line)]


def verilate(verilator, env, obj, sources, threads, log, extra=()):
    with log.open('w') as f:
        subprocess.run([str(verilator), '--cc', '--exe', '--build', '-j', '4', '--trace-vtr', '--timing',
                        '--threads', str(threads), '--top-module', 'tb', '--Mdir', str(obj), *map(str, sources),
                        '-o', 'sim', *extra], env=env, stdout=f, stderr=subprocess.STDOUT, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--verilator', type=Path, required=True)
    parser.add_argument('--out', type=Path, default=ROOT / 'bench/build/verilator-clocks')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    verilator = args.verilator.resolve()
    env = dict(os.environ, VTR_INCLUDE=str(ROOT / 'core/vtr-capi/include'), VTR_LIBDIR=str(ROOT / 'target/release'))
    cli = ROOT / 'target/release/vtr'

    # A design naming nothing from the package: no package code in the model.
    obj = out / 'obj_unused'
    subprocess.run([str(verilator), '--cc', '--trace-vtr', '--top-module', 'tb', '--Mdir', str(obj), str(HERE / 'unused.sv')],
                   env=env, stdout=subprocess.DEVNULL, check=True)
    for f in obj.iterdir():
        if f.suffix in ('.cpp', '.h', '.json', '.d', '.dat') and re.search(r'vtr_trace|vtr_clock|vtr_now', f.read_text(errors='replace')):
            raise AssertionError(f'{f} references the unused vtr_trace package')
    print('unused package: no package code in the model')

    for threads in [1, 2]:
        obj = out / f'obj_{threads}'
        verilate(verilator, env, obj, [HERE / 'top.sv', HERE / 'main.cpp'], threads, out / f'build_{threads}.log')
        runs = {}
        for mode in ['normal', 'late_open', 'nosignals']:
            run = out / f'{mode}_{threads}'
            run.mkdir(exist_ok=True)
            result = subprocess.run([str(obj / 'sim'), '+' + mode], cwd=run, capture_output=True, text=True, timeout=60)
            (run / 'console.txt').write_text(result.stdout + result.stderr)
            assert result.returncode == 0, (mode, result)
            runs[mode] = run / 'clocks.vtr'

        trace = runs['normal']
        got = clocks(cli, trace)
        assert set(got) == {'TOP.tb.core_clk', 'TOP.tb.bus_clk', 'TOP.tb.g.odd_clk'}, got
        core, core_open = got['TOP.tb.core_clk']
        bus, bus_open = got['TOP.tb.bus_clk']
        assert core_open and bus_open, got
        # Slower, then faster, then slower again: four speeds, one stretch each.
        assert [p for _, _, p in core] == [334, 500, 200, 666], core
        # Gated for 9 ns: the ns period is converted to 2000 ps.
        assert [p for _, _, p in bus] == [2000, 2000], bus
        assert got['TOP.tb.g.odd_clk'] == ([], False), got
        for path, stretches in [('TOP.tb.core_clk', core), ('TOP.tb.bus_clk', bus)]:
            want = rising_edges(cli, trace, path)
            assert stretch_edges(stretches) == want, (path, stretches, want)
        warns = warnings(cli, trace)
        assert any('TOP.tb.g.odd_clk: period 1500 x 10^-15 s is not a whole number of file units' in w for w in warns), warns
        assert any('TOP.tb.core_clk: vtr_clock_run on a running clock' in w for w in warns), warns
        assert 'Warning-VTRTRACE' in (trace.parent / 'console.txt').read_text()

        # Opened at 5 ns: the clocks declared and started before are replayed from their first edge.
        late = clocks(cli, runs['late_open'])
        assert late['TOP.tb.core_clk'] == got['TOP.tb.core_clk'], late
        assert late['TOP.tb.bus_clk'] == got['TOP.tb.bus_clk'], late
        waves = rising_edges(cli, runs['late_open'], 'TOP.tb.core_clk')
        assert waves and waves[0] >= 5000 and set(waves) <= set(stretch_edges(core)), waves

        # Without the model's signals the file still holds the clocks, under created scopes.
        bare = clocks(cli, runs['nosignals'])
        assert bare == got, bare
        print(f'{threads} threads: {len(stretch_edges(core))} core and {len(stretch_edges(bus))} bus edges match the '
              f'waveforms; late open, signal-free open, scopes, units and misuse reports verified')


if __name__ == '__main__':
    main()
