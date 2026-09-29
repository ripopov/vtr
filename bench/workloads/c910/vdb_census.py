#!/usr/bin/env python3
"""Census of an RTL VDB v2 JSON export, for the VDB vision (docs/VDB_rethinked.html).

Measures what the page's "Today" and "Format" sections quote for openC910:

* the size of each top-level part of the export;
* instances, modules and distinct specializations: an instance's processes,
  symbols, ports and the connections of its children, with every name made
  relative to the instance, hashed;
* how many processes and register targets the temporal tracer can evaluate
  (no Unsupported statement or expression anywhere in the body), and the
  unsupported statements by reason;
* the size of the per-specialization bodies as JSON and compressed one body at
  a time with zstd -3, of the source index and of the instance tree, and of the
  source files named by the export (an estimate of the proposed container,
  which the page labels as such);
* case statements in the sources whose subject is named like a state register.

    vdb_census.py bench/workloads/gen/c910_build/obj_vtr/Vtop.vdb.json

The opening cost quoted beside it is `/usr/bin/time -v target/release/vtr-vdb
check <export> bench/results/latest/c910_coremark_sim.vtr --prefix TOP`.
Needs the `zstd` command.
"""
import argparse
import collections
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys

MIB = 1 << 20


def zstd_size(data: bytes) -> int:
    return len(subprocess.run(['zstd', '-3', '-c', '-q'], input=data, capture_output=True, check=True).stdout)


def unsupported_statements(stmt, out):
    kind = stmt.get('kind')
    if kind == 'Unsupported':
        out.append(stmt.get('reason', ''))
    elif kind == 'Sequence':
        for s in stmt['statements']:
            unsupported_statements(s, out)
    elif kind == 'If':
        unsupported_statements(stmt['yes'], out)
        unsupported_statements(stmt['no'], out)


def has_unsupported(node) -> bool:
    if isinstance(node, dict):
        if node.get('kind') == 'Unsupported':
            return True
        return any(has_unsupported(v) for v in node.values())
    if isinstance(node, list):
        return any(has_unsupported(v) for v in node)
    return False


def relative(obj, prefix: str) -> str:
    text = json.dumps(obj, sort_keys=True)
    text = text.replace('"' + prefix + '.', '"@.').replace('"' + prefix + '"', '"@"')
    return re.sub(r'"id": \d+', '"id": 0', text)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('export')
    args = ap.parse_args()
    if not shutil.which('zstd'):
        sys.exit('vdb_census.py needs the zstd command')
    size = os.path.getsize(args.export)
    with open(args.export) as f:
        d = json.load(f)
    print(f'file: {size:,} bytes')
    for key, value in d.items():
        if isinstance(value, (list, dict)):
            print(f'  {key:<14} {len(json.dumps(value)) / MIB:9.1f} MiB  ({len(value):,} entries)')

    insts = d['instances']
    parent = {i['path']: i['parent'] for i in insts}
    by_owner_p, by_owner_s, by_parent_c = (collections.defaultdict(list) for _ in range(3))
    for p in d['processes']:
        by_owner_p[p['owner']].append(p)
    for s in d['symbols'].values():
        by_owner_s[s['owner']].append(s)
    for c in d['connections']:
        by_parent_c[parent.get(c['instance'])].append(c)
    bodies, spec_of = {}, {}
    for i in insts:
        path = i['path']
        body = relative({'p': by_owner_p.get(path, []), 's': by_owner_s.get(path, []),
                         'c': by_parent_c.get(path, []), 'ports': i.get('ports', [])}, path)
        key = hashlib.sha256(body.encode()).hexdigest()[:16]
        bodies.setdefault(key, body)
        spec_of[path] = key
    gates = sum(1 for i in insts if i['definition'] == 'gated_clk_cell')
    print(f'instances {len(insts):,}, modules {len({i["definition"] for i in insts}):,}, '
          f'distinct specializations {len(bodies):,}, gated_clk_cell instances {gates:,}')

    procs = collections.Counter()
    targets = collections.Counter()
    reasons = collections.Counter()
    for p in d['processes']:
        stmts = []
        if p.get('body'):
            unsupported_statements(p['body'], stmts)
        for r in stmts:
            reasons[r.split(':')[0]] += 1
        ok = p['mode'] != 'unsupported' and not has_unsupported(p.get('body'))
        procs[(p['mode'], p['origin'], ok)] += 1
        targets[(p['mode'] == 'comb', ok)] += len(p.get('targets', []))
    comb_ok, comb = procs[('comb', 'rtl', True)], procs[('comb', 'rtl', True)] + procs[('comb', 'rtl', False)]
    reg_ok = targets[(False, True)]
    reg = targets[(False, True)] + targets[(False, False)]
    print(f'combinational RTL processes evaluable: {comb_ok:,} of {comb:,} ({100 * comb_ok / comb:.1f}%)')
    print(f'register targets in evaluable processes: {reg_ok:,} of {reg:,} ({100 * reg_ok / reg:.1f}%)')
    for reason, n in reasons.most_common():
        print(f'  unsupported statements: {n:>7,}  {reason}')

    raw = sum(len(b) for b in bodies.values())
    packed = sum(zstd_size(b.encode()) for b in bodies.values())
    tree = json.dumps([[i['path'].rsplit('.', 1)[-1], i['parent'], spec_of[i['path']]] for i in insts]).encode()
    tokens = json.dumps(d.get('source_index', {})).encode()
    work = d['elaboration']['work_dir']
    text = b''
    for s in d['sources']:
        path = s['path'] if os.path.isabs(s['path']) else os.path.join(work, s['path'])
        with open(path, 'rb') as f:
            text += f.read()
    parts = {'specialization bodies': (raw, packed), 'source index': (len(tokens), zstd_size(tokens)),
             'instance tree': (len(tree), zstd_size(tree)), 'source files': (len(text), zstd_size(text))}
    for name, (a, b) in parts.items():
        print(f'{name:<22} {a / MIB:8.2f} MiB JSON or text, {b / MIB:6.2f} MiB zstd -3')
    print(f'estimated self-contained container: {sum(b for _, b in parts.values()) / MIB:.1f} MiB')

    subject = re.compile(r'\bcase[zx]?\s*\(\s*([A-Za-z_]\w*)\s*(?:\[[^\]]*\])?\s*\)')
    cases, named = 0, set()
    for s in d['sources']:
        path = s['path'] if os.path.isabs(s['path']) else os.path.join(work, s['path'])
        with open(path, errors='replace') as f:
            for m in subject.finditer(f.read()):
                cases += 1
                if re.search(r'(state|_st$|_cur$|cur_st|_fsm)', m.group(1), re.I):
                    named.add((os.path.basename(path), m.group(1)))
    print(f'case statements {cases:,}; subjects named like state registers: {len(named)} (file, subject) pairs')


if __name__ == '__main__':
    main()
