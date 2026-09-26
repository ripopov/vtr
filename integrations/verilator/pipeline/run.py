#!/usr/bin/env python3
"""Build and verify pipeline tracing through the vtr_trace package (docs/c910-verilator-tx-stream.html).

demo_core (core.sv) is traced by demo_tracer (tracer.sv), attached with bind
(bind.sv); the tracer exercises every tracker call. In one and two thread modes
this checks: every transaction, stage, status, attribute, relation, parent and
time against the exact expected list below; that stage boundaries fall on edges
of the declared clock; that both streams name that clock; that the waveforms
equal those of the same design built without the tracer; the misuse warning of
the script's deliberate stale key; and that a signal-free open records the same
transactions. --update-example writes the recording to
volna/volna/examples/pipeline_demo.vtr.
"""
import argparse
import os
from pathlib import Path
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent

# One line per transaction: identity (its label, or stream@begin), [begin..end] status,
# attributes, stages "name@lane begin-end" in order, events, parent and relations.
EXPECTED = """\
pipeline 0100 add a0,a0,a1 [15..75] ok pc=256 unit=alu | F 15-25, D 25-35, Q 35-45, X 45-55, C 55-65, R 65-75 | -> wakeup 0104 lw a2,0(a0)
pipeline 0104 lw a2,0(a0) [25..95] ok pc=260 unit=lsu | F 25-35, D 35-45, Q 45-55, X 55-75, C 75-85, R 85-95 | replay@65
pipeline 0108 add a3,a2,a1 [35..125] ok pc=264 unit=alu | F 35-65, S@stall 45-65, D 65-85, Q 85-95, X 95-105, C 105-115, R 115-125
pipeline 010c sub a4,a3,a0 [45..125] ok pc=268 unit=alu | F 45-75, D 75-85, Q 85-95, X 95-105, C 105-115, R 115-125
bus bus@45 [45..75] ok addr=8192 | A 45-65, D 65-75 | parent=0104 lw a2,0(a0)
pipeline 0110 beq a4,zero,0x200 [75..135] ok pc=272 unit=alu | F 75-85, D 85-95, Q 95-105, X 105-115, C 115-125, R 125-135
pipeline 0114 addi a7,a7,1 [85..115] aborted pc=276 unit=alu | F 85-95, D 95-115
pipeline 0118 addi a7,a7,1 [95..115] aborted pc=280 unit=alu | F 95-105, D 105-115
pipeline 011c addi a7,a7,1 [105..115] aborted pc=284 unit=alu | F 105-115
pipeline 0200 xor a5,a5,a5 [115..175] error pc=512 unit=alu | F 115-125, D 125-135, Q 135-145, X 145-155, C 155-165, R 165-175
pipeline 0204 lw a6,8(a0) [125..145] aborted pc=516 unit=lsu | F 125-135, D 135-145
pipeline 0208 addi a7,a7,1 [135..145] aborted pc=520 unit=alu | F 135-145
bus bus@135 [135..175] aborted addr=12288 | A 135-175 | parent=0200 xor a5,a5,a5
pipeline 0300 fence.i [145..200] open pc=768 unit=alu | F 145-155, D 155-200
"""
WARNING = 'vtr_trace: TOP.tb.core.pipeline: stage on a key that names no item (1 time, first rob 1)'
STREAMS = {'TOP.tb.core.pipeline': 'pipeline', 'TOP.tb.core.bus': 'bus'}


def transactions(cli, trace):
    """The tracer's transactions from `vtr tx`, in id (begin) order."""
    out, cur = [], None
    for line in subprocess.check_output([str(cli), 'tx', str(trace), '--max', '1000'], text=True).splitlines():
        m = re.match(r'tx (\d+) (\S+)/\S+ \[(\d+) \.\. (\d+)\] status=(\w+) kind=\w+(?: parent=(\d+))?$', line)
        if m:
            cur = None
            if m[2] in STREAMS:
                cur = {'id': int(m[1]), 'stream': STREAMS[m[2]], 'begin': int(m[3]), 'end': int(m[4]),
                       'status': m[5], 'parent': int(m[6]) if m[6] else None, 'attrs': {}, 'stages': [],
                       'events': [], 'rel': []}
                out.append(cur)
            continue
        if cur is None:
            continue
        if m := re.match(r'    stage (\S+) lane=(\S*) \[(\d+) \.\. (\d+|open)\]', line):
            cur['stages'].append((m[1], m[2], int(m[3]), m[4]))
        elif m := re.match(r'    event @(\d+) (\S+)', line):
            cur['events'].append(f'{m[2]}@{m[1]}')
        elif m := re.match(r'    (\S+) = (.*)$', line):
            cur['attrs'][m[1]] = m[2].strip('"')
    for tx in out:
        for line in subprocess.check_output([str(cli), 'tx', str(trace), '--id', str(tx['id'])], text=True).splitlines():
            if m := re.match(r'    -> (\S+) (\d+)', line):
                tx['rel'].append((m[1], int(m[2])))
    return sorted(out, key=lambda t: t['id'])


def canonical(txs):
    name = {t['id']: t['attrs'].get('vtr.label', f"{t['stream']}@{t['begin']}") for t in txs}
    lines = []
    for t in txs:
        attrs = ' '.join(f'{k}={v}' for k, v in t['attrs'].items() if k != 'vtr.label')
        parts = [f"{t['stream']} {name[t['id']]} [{t['begin']}..{t['end']}] {t['status']} {attrs}",
                 ', '.join(f"{s}{'@' + l if l else ''} {b}-{e}" for s, l, b, e in t['stages'])]
        extra = t['events'] + ([f"parent={name[t['parent']]}"] if t['parent'] else [])
        extra += [f'-> {k} {name[to]}' for k, to in t['rel']]
        if extra:
            parts.append(' '.join(extra))
        lines.append(' | '.join(parts))
    return '\n'.join(lines) + '\n'


def clock_edges(cli, trace):
    out = subprocess.check_output([str(cli), 'clocks', str(trace)], text=True).splitlines()
    assert out[0].startswith('clock 0 TOP.tb.clk:'), out
    m = re.match(r'\s+\[(\d+) \.\. (\d+)\] period (\d+)', out[1])
    b, e, p = map(int, m.groups())
    return set(range(b, e + 1, p))


def verilate(verilator, env, obj, sources, threads, log):
    with log.open('w') as f:
        subprocess.run([str(verilator), '--cc', '--exe', '--build', '-j', '4', '--trace-vtr', '--timing',
                        '--threads', str(threads), '--top-module', 'tb', '--Mdir', str(obj), *map(str, sources),
                        '-o', 'sim'], env=env, stdout=f, stderr=subprocess.STDOUT, check=True)


def simulate(obj, run, *plusargs):
    run.mkdir(parents=True, exist_ok=True)
    result = subprocess.run([str(obj / 'sim'), *plusargs], cwd=run, capture_output=True, text=True, timeout=60)
    (run / 'console.txt').write_text(result.stdout + result.stderr)
    assert result.returncode == 0, (run, result)
    return run / 'pipeline.vtr'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--verilator', type=Path, required=True)
    parser.add_argument('--out', type=Path, default=ROOT / 'bench/build/verilator-pipeline')
    parser.add_argument('--update-example', action='store_true',
                        help='copy the one-thread recording to volna/volna/examples/pipeline_demo.vtr')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    verilator = args.verilator.resolve()
    env = dict(os.environ, VTR_INCLUDE=str(ROOT / 'core/vtr-capi/include'), VTR_LIBDIR=str(ROOT / 'target/release'))
    cli = ROOT / 'target/release/vtr'
    for f in ['vtr_trace.sv', 'vtr_trace_dpi.hpp', 'vtr_track.hpp']:
        installed = Path(subprocess.check_output([str(verilator), '--getenv', 'VERILATOR_ROOT'], text=True).strip())
        if (installed / 'include/vtr' / f).read_bytes() != (ROOT / 'core/vtr-capi/include' / f).read_bytes():
            raise SystemExit(f'{installed}/include/vtr/{f} differs from core/vtr-capi/include/{f}; rerun build.sh')

    design = [HERE / 'core.sv', HERE / 'main.cpp']
    traced = [HERE / 'core.sv', HERE / 'tracer.sv', HERE / 'bind.sv', HERE / 'main.cpp']
    verilate(verilator, env, out / 'obj_base', design, 1, out / 'build_base.log')
    base = simulate(out / 'obj_base', out / 'base')
    base_waves = subprocess.check_output([str(cli), 'dump', str(base)], text=True)
    assert not transactions(cli, base), 'the untraced design recorded pipeline transactions'

    for threads in [1, 2]:
        obj = out / f'obj_{threads}'
        verilate(verilator, env, obj, traced, threads, out / f'build_{threads}.log')
        trace = simulate(obj, out / f'normal_{threads}')
        txs = transactions(cli, trace)
        got = canonical(txs)
        if got != EXPECTED:
            import difflib
            raise AssertionError(''.join(difflib.unified_diff(EXPECTED.splitlines(True), got.splitlines(True),
                                                              'expected', f'{threads} threads')))
        edges = clock_edges(cli, trace)
        for t in txs:
            times = [t['begin']] + [b for _, _, b, _ in t['stages']]
            if t['status'] != 'open':
                times += [t['end']] + [int(e) for _, _, _, e in t['stages']]
            assert set(times) <= edges, (t, sorted(set(times) - edges))
        hier = subprocess.check_output([str(cli), 'hier', str(trace)], text=True)
        for stream, kind in [('pipeline', 'PIPELINE'), ('bus', 'BUS')]:
            assert re.search(rf'^      {stream} \[stream {kind}\]\n        @vtr\.clock = "TOP\.tb\.clk"$', hier, re.M), hier
        assert subprocess.check_output([str(cli), 'dump', str(trace)], text=True) == base_waves, \
            'the tracer changed the recorded waveforms'
        log = subprocess.check_output([str(cli), 'log', str(trace)], text=True)
        warns = [line.split('simulation_log: ', 1)[1] for line in log.splitlines() if ' warn ' in line]
        assert warns == [WARNING], warns
        assert f'%Warning-VTRTRACE: {WARNING}' in (trace.parent / 'console.txt').read_text()

        bare = simulate(obj, out / f'nosignals_{threads}', '+nosignals')
        assert canonical(transactions(cli, bare)) == EXPECTED, 'signal-free open recorded other transactions'
        assert clock_edges(cli, bare) == edges
        print(f'{threads} threads: {len(txs)} transactions match exactly; stages on clock edges, clock names, '
              f'unchanged waveforms, stale-key warning and signal-free open verified')
        if threads == 1 and args.update_example:
            dest = ROOT / 'volna/volna/examples/pipeline_demo.vtr'
            dest.unlink(missing_ok=True)
            shutil.copyfile(trace, dest)
            print(f'wrote {dest.relative_to(ROOT)}')


if __name__ == '__main__':
    main()
