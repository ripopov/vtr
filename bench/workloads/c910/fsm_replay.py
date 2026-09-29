#!/usr/bin/env python3
"""Replay C910's physical-register lifecycle state machine against a recording.

The VDB vision (docs/VDB_rethinked.html) uses one state machine as its demo:
`lifecycle_cur_state` of `ct_rtu_pst_preg_entry.v`, instantiated for each of
the 95 physical registers under x_ct_rtu_pst_preg. This script does what the
proposed `fsm_replay` query would do, by hand: it reads the state register and
every input of the machine's guards for each entry through `vtr changes`, and
evaluates the source's transition logic before every rising clock edge (even
time units in this harness, sampled at T - 1 as the flip-flop sees them).

For every recorded state change it names the source line whose branch fired,
and it reports any edge where the prediction and the recording disagree. The
only disagreements are the edges at which the reset deasserts in the same time
step as the clock edge, where the trace cannot say which came first.

    fsm_replay.py <capture.vtr> [--vtr path/to/vtr] [--demo-json out.json]
        [--window T0:T1] [--source path/to/ct_rtu_pst_preg_entry.v]

--demo-json writes the literal embedded in the page (the `const D = ...` of its
script): per-entry counts by source line, time in each state, every state
change of every entry inside --window, the flush pulses there, the source
excerpt and the facts the page quotes for preg15. The page's literal was made
from the benchmark recording with

    fsm_replay.py bench/results/latest/c910_coremark.vtr --window 159400:160200 --demo-json demo.json
"""
import argparse
import bisect
import collections
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
SCOPE = ('TOP.top.x_soc.x_cpu_sub_system_axi.x_rv_integration_platform.x_cpu_top.x_ct_top_0.x_ct_core'
         '.x_ct_rtu_top.x_ct_rtu_pst_preg.x_ct_rtu_pst_entry_preg{}.')
STATE = 'lifecycle_cur_state [4:0]'
INPUTS = ['rtu_yy_xx_flush', 'x_release_vld', 'wb_cur_state_wb_masked', 'retire_vld', 'x_dealloc_vld',
          'create_vld', 'cpurst_b', 'ifu_xx_sync_reset', 'x_reset_mapped']
NAMES = {'00001': 'DEALLOC', '00010': 'WF_ALLOC', '00100': 'ALLOC', '01000': 'RETIRE', '10000': 'RELEASE'}
ORDER = list(NAMES.values())
KEYS = ['None>DEALLOC@270', 'RETIRE>DEALLOC@270', 'DEALLOC>RETIRE@272', 'DEALLOC>WF_ALLOC@288', 'WF_ALLOC>DEALLOC@292',
        'WF_ALLOC>ALLOC@294', 'ALLOC>DEALLOC@298', 'ALLOC>DEALLOC@300', 'ALLOC>RELEASE@302', 'ALLOC>RETIRE@304',
        'RETIRE>DEALLOC@308', 'RETIRE>RELEASE@310', 'RELEASE>DEALLOC@314']
SOURCE = ROOT / 'ext/pulp-c910/vendor/thead_openc910/C910_RTL_FACTORY/gen_rtl/rtu/rtl/ct_rtu_pst_preg_entry.v'


def changes(vtr, trace, path):
    out = subprocess.run([vtr, 'changes', trace, path], capture_output=True, text=True, check=True).stdout
    rows = [line.split('\t') for line in out.splitlines() if line and not line.startswith('...')]
    return [int(t) for t, _ in rows], [v for _, v in rows]


def at(hist, t):
    i = bisect.bisect_right(hist[0], t) - 1
    return hist[1][i] if i >= 0 else None


def predict(g, t):
    """The state after the rising edge at t and the source line that decides it (ct_rtu_pst_preg_entry.v)."""
    if at(g['cpurst_b'], t) == '0':
        return 'DEALLOC', 270
    high = lambda name: at(g[name], t - 1) == '1'
    if high('ifu_xx_sync_reset'):
        return ('RETIRE' if high('x_reset_mapped') else 'DEALLOC'), 272
    cur = NAMES.get(at(g[STATE], t - 1))
    flush, release, wb = high('rtu_yy_xx_flush'), high('x_release_vld'), high('wb_cur_state_wb_masked')
    if cur == 'DEALLOC':
        return ('WF_ALLOC', 288) if high('x_dealloc_vld') and not flush else ('DEALLOC', 290)
    if cur == 'WF_ALLOC':
        return ('DEALLOC', 292) if flush else ('ALLOC', 294) if high('create_vld') else ('WF_ALLOC', 296)
    if cur == 'ALLOC':
        if flush:
            return 'DEALLOC', 298
        if release:
            return ('DEALLOC', 300) if wb else ('RELEASE', 302)
        return ('RETIRE', 304) if high('retire_vld') else ('ALLOC', 306)
    if cur == 'RETIRE':
        if release:
            return ('DEALLOC', 308) if wb else ('RELEASE', 310)
        return 'RETIRE', 312
    if cur == 'RELEASE':
        return ('DEALLOC', 314) if wb else ('RELEASE', 316)
    return 'DEALLOC', 318


def replay(g, end):
    """Counts per (from > to @ line), ambiguous edges, and the number of edges checked."""
    state = g[STATE]
    times = set()
    for hist in g.values():
        times.update(hist[0])
    edges = {t + 1 if (t + 1) % 2 == 0 else t + 2 for t in times} | set(g['cpurst_b'][0])
    edges |= {t for t in times if t % 2 == 1}
    counts, ambiguous, checked = collections.Counter(), [], 0
    for t in sorted(edges):
        if t < 2 or t > end:
            continue
        cur = NAMES.get(at(state, t - 1))
        if t % 2 == 1:  # between clock edges only the asynchronous reset acts
            if at(g['cpurst_b'], t) != '0':
                continue
            nxt, line = 'DEALLOC', 270
        else:
            nxt, line = predict(g, t)
        actual = NAMES.get(at(state, t))
        checked += 1
        if cur == nxt and actual == cur:
            continue
        if actual != nxt:
            ambiguous.append((t, cur, nxt, actual))
        else:
            counts[f'{cur}>{nxt}@{line}'] += 1
    return counts, ambiguous, checked


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('trace')
    ap.add_argument('--vtr', default=str(ROOT / 'target/release/vtr'))
    ap.add_argument('--demo-json')
    ap.add_argument('--window', default='159400:160200')
    ap.add_argument('--source', default=str(SOURCE))
    args = ap.parse_args()
    info = subprocess.run([args.vtr, 'info', args.trace], capture_output=True, text=True, check=True).stdout
    end = int(next(l for l in info.splitlines() if l.startswith('time range:')).split('..')[1])
    entries = {}
    for i in range(1, 96):
        entries[i] = {name: changes(args.vtr, args.trace, SCOPE.format(i) + name) for name in [STATE] + INPUTS}
    total, per, ambiguous, checked, recorded = collections.Counter(), {}, [], 0, 0
    for i, g in entries.items():
        c, amb, n = replay(g, end)
        per[i], checked = c, checked + n
        total.update(c)
        ambiguous += [(i,) + a for a in amb]
        recorded += len(g[STATE][0]) - 1
    predicted = sum(total.values())
    print(f'{len(entries)} entries, {checked:,} edges checked, {recorded:,} recorded changes, {predicted:,} predicted, '
          f'{len(ambiguous)} ambiguous at t = {sorted({a[1] for a in ambiguous})}')
    for key in KEYS:
        print(f'  {total[key]:>9,}  {key}')

    if not args.demo_json:
        return
    w0, w1 = map(int, args.window.split(':'))
    code = {v: k for k, v in enumerate(ORDER)}
    rows, occ = {}, {}
    for i, g in entries.items():
        ts, vs = g[STATE]
        k = bisect.bisect_right(ts, w0) - 1
        rows[i] = [[w0, code[NAMES[vs[k]]]]] + [[t, code[NAMES[v]]] for t, v in zip(ts[k + 1:], vs[k + 1:]) if t < w1]
        o = [0] * len(ORDER)
        for j, (t, v) in enumerate(zip(ts, vs)):
            if v in NAMES:
                o[code[NAMES[v]]] += (ts[j + 1] if j + 1 < len(ts) else end + 1) - t
        occ[i] = o
    fts, fvs = entries[1]['rtu_yy_xx_flush']
    flush = [[t, fts[j + 1] if j + 1 < len(fts) else end] for j, (t, v) in enumerate(zip(fts, fvs))
             if v == '1' and t < w1 and (fts[j + 1] if j + 1 < len(fts) else end) > w0]
    # preg15: when it last entered RETIRE and through which line, and its last release request.
    ts, vs = entries[15][STATE]
    enter = ts[-1]
    via = predict(entries[15], enter)[1]
    rts, rvs = entries[15]['x_release_vld']
    highs = [j for j, v in enumerate(rvs) if v == '1']
    last = [rts[highs[-1]], rts[highs[-1] + 1]]
    lines = pathlib.Path(args.source).read_text().split('\n')
    source = [[n, lines[n - 1].rstrip()] for n in list(range(185, 190)) + list(range(255, 320))]
    demo = {'window': [w0, w1], 'end': end, 'keys': KEYS,
            'counts': {i: [per[i][k] for k in KEYS] for i in entries}, 'occ': occ, 'rows': rows, 'flush': flush,
            'source': source,
            'preg15': {'enter': enter, 'via': via, 'releaseLast': last, 'state': NAMES[vs[-1]]},
            'replay': {'edges': checked, 'recorded': recorded, 'ambiguous': len(ambiguous),
                       'ambiguousAt': sorted({a[1] for a in ambiguous})}}
    pathlib.Path(args.demo_json).write_text(json.dumps(demo, separators=(',', ':')))
    print(f'wrote {args.demo_json}')


if __name__ == '__main__':
    sys.exit(main())
