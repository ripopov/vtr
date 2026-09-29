#!/usr/bin/env python3
"""Regenerate the recorded data embedded in docs/VDB_Fable.html.

Verilates soc.sv with the pinned Verilator's --trace-vtr (a real VTR recording
and its RTL VDB v2 companion), checks the pair with the vtr-vdb CLI, and
splices three facts into the page on one line each: the VDB document (paths
made relative), every recorded value change, and the design source. The page
and its headless test read nothing else.

    python3 docs/vdb-fable/build.py [--verilator PATH] [--out DIR]
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
PAGE = ROOT / 'docs/VDB_Fable.html'


def run(cmd, **kw):
    return subprocess.run([str(c) for c in cmd], text=True, capture_output=True, check=True, **kw).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--verilator', type=Path, default=ROOT / 'bench/build/verilator/install/bin/verilator')
    parser.add_argument('--out', type=Path, default=ROOT / 'bench/build/vdb-fable')
    parser.add_argument('--data-only', action='store_true', help='write demo.json beside the build instead of splicing the page')
    args = parser.parse_args()
    out = args.out.resolve()
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    lib = out / 'lib'
    lib.mkdir()
    shutil.copy2(ROOT / 'target/release/libvtr.a', lib / 'libvtr.a')
    env = dict(os.environ, VTR_INCLUDE=str(ROOT / 'core/vtr-capi/include'), VTR_LIBDIR=str(lib))
    run([args.verilator.resolve(), '--cc', '--exe', '--build', '-j', '4', '--top-module', 'top', '--prefix', 'Vtop',
         '--Mdir', out / 'obj', '--trace-vtr', '-Wno-fatal', HERE / 'soc.sv', HERE / 'main.cpp', '-o', 'sim'], env=env, cwd=out)
    trace = out / 'trace.vtr'
    run([out / 'obj/sim', trace], cwd=out)
    cli = ROOT / 'target/release/vtr-vdb'
    vtr = ROOT / 'target/release/vtr'
    check = run([cli, 'check', out / 'trace.vdb.json', trace])
    assert 'structural match only' not in check, check

    vdb = json.loads((out / 'trace.vdb.json').read_text())
    # Relative paths: the page is the only reader, and it must not depend on this machine.
    src = str(HERE / 'soc.sv')
    text = json.dumps(vdb, separators=(',', ':'))
    text = text.replace(src, 'soc.sv').replace(str(out), '.').replace(str(HERE), 'docs/vdb-fable').replace(str(ROOT), '.')
    assert '/home/' not in text and '/tmp/' not in text, 'a machine path survived'
    vdb = json.loads(text)
    vdb['sources'] = [s for s in vdb['sources'] if s['path'] == 'soc.sv']
    vdb['source_index']['files'] = [f for f in vdb['source_index']['files'] if f['path'] == 'soc.sv']
    vdb['elaboration']['work_dir'] = '.'
    vdb['elaboration']['files'] = ['soc.sv']
    vdb['options'] = [o for o in vdb['options'] if not o.startswith(('--Mdir', '-o'))]

    # Every recorded var by path, with the signal it names: aliases (a port and
    # the net it connects to) share one signal, and the dump prints one name per signal.
    vars_by_path, stack = {}, []
    for line in run([vtr, 'hier', trace, '--vars']).splitlines():
        m = re.match(r'^( *)(\S+)(?: \[[^\]]*\])? (?:\[(\w+)[^\]]*\]|: .*sig#(\d+))', line)
        if not m:
            continue
        depth = len(m.group(1)) // 2
        del stack[depth:]
        if m.group(4) is not None:
            vars_by_path['.'.join(stack + [m.group(2)])] = int(m.group(4))
        else:
            stack.append(m.group(2))
    signals, time = {}, 0
    for line in run([vtr, 'dump', trace]).splitlines():
        if line.startswith('#'):
            time = int(line[1:])
            continue
        value, path = line.split(' ', 1)
        path = re.sub(r' \[\d+:\d+\]$', '', path)
        signals.setdefault(str(vars_by_path[path]), []).append([time, value])
    info = run([vtr, 'info', trace])
    end = int(re.search(r'time range:\s+\d+ \.\. (\d+)', info).group(1))
    enums = {}
    for m in re.finditer(r'^\s+(\S+) \[[^\]]+\] :.*\n\s+@enum_table = (\d+)', run([vtr, 'hier', trace, '--vars']), re.M):
        enums[m.group(1)] = int(m.group(2))
    # The enum literals as the source declares them: the trace keeps them as an
    # enum table, the RTL VDB v2 export drops them (see the page's "what is lost").
    source = (HERE / 'soc.sv').read_text()
    enum_types = {m.group(2): [[n, int(v)] for n, v in re.findall(r"(\w+)\s*=\s*\d+'d(\d+)", m.group(1))]
                  for m in re.finditer(r'typedef enum[^{]*\{([^}]*)\}\s*(\w+);', source)}
    data = {'vdb': vdb, 'trace': {'unit': 'ps', 'end': end, 'signals': signals, 'vars': vars_by_path, 'enum_tables': enums},
            'enums': enum_types, 'source': source}
    blob = json.dumps(data, separators=(',', ':'), ensure_ascii=False)
    (out / 'demo.json').write_text(blob)
    print(f'{len(vdb["instances"])} instances, {len(vdb["symbols"])} symbols, {len(vdb["processes"])} processes, '
          f'{sum(len(c) for c in signals.values())} changes over {len(signals)} signals and {len(vars_by_path)} vars, {len(blob)} bytes')
    if args.data_only:
        return
    page = PAGE.read_text()
    marker = 'const DATA = '
    lines = page.split('\n')
    at = [i for i, l in enumerate(lines) if l.startswith(marker)]
    assert len(at) == 1, 'the page holds exactly one DATA line'
    lines[at[0]] = marker + blob + ';'
    PAGE.write_text('\n'.join(lines))
    print(f'spliced into {PAGE.relative_to(ROOT)}')


if __name__ == '__main__':
    main()
