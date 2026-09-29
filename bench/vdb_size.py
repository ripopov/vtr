#!/usr/bin/env python3
"""Measure an RTL VDB v2 JSON export: bytes per section, per-definition
redundancy and the zstd size, the numbers docs/VDB_Fable.html quotes.

    python3 bench/vdb_size.py bench/workloads/gen/c910_build/obj_vtr/Vtop.vdb.json

The per-definition figures replace each item's owner path by a placeholder and
count distinct texts per module definition: an upper bound on what a store
keyed by definition instead of instance would hold, before any encoding.
"""
import json
import subprocess
import sys
import time


def main(path):
    started = time.time()
    with open(path) as f:
        db = json.load(f)
    parse = time.time() - started
    size = sum(len(json.dumps(v, separators=(',', ':'))) for v in db.values())
    print(f'{path}: {size / 1e6:.1f} MB of JSON, parsed by Python in {parse:.1f} s')
    for key in ('instances', 'symbols', 'connections', 'processes', 'source_index'):
        if key in db:
            print(f'  {key:14s}{len(json.dumps(db[key], separators=(",", ":"))) / 1e6:8.1f} MB')
    inst = {i['path']: i for i in db['instances']}
    defs = {}
    for i in db['instances']:
        defs.setdefault(i['definition'], []).append(i['path'])
    print(f'  {len(db["instances"])} instances of {len(defs)} definitions; the largest definition has '
          f'{max(len(v) for v in defs.values())} instances')
    print(f'  {len(db["symbols"])} symbols, {len(db["connections"])} connections, {len(db["processes"])} processes')
    if db.get('source_index'):
        files = db['source_index']['files']
        print(f'  source index: {len(files)} files, {sum(len(x["tokens"]) // 5 for x in files)} tokens')

    def strip(obj, owner):
        s = json.dumps(obj, separators=(',', ':'))
        return s.replace('"' + owner + '.', '"@.').replace('"' + owner + '"', '"@"')

    for name, items, owner_of in (
            ('processes', db['processes'], lambda p: p['owner']),
            ('symbols', db['symbols'].values(), lambda s: s['owner']),
            ('connections', db['connections'], lambda c: c['instance'].rsplit('.', 1)[0])):
        unique, total = set(), 0
        for it in items:
            owner = owner_of(it)
            s = strip(it, owner)
            total += len(s)
            unique.add((inst[owner]['definition'] if owner in inst else '?', s[s.find('"origin"'):] if name == 'processes' else s))
        print(f'  {name}: {len(unique)} distinct per definition of {sum(1 for _ in items)}, '
              f'{sum(len(s) for _, s in unique) / 1e6:.1f} MB of {total / 1e6:.1f} MB')
    try:
        z = subprocess.run(['zstd', '-3', '-c', path], capture_output=True, check=True).stdout
        print(f'  zstd -3 of the file: {len(z) / 1e6:.1f} MB')
    except (OSError, subprocess.CalledProcessError):
        print('  zstd not available')


if __name__ == '__main__':
    main(sys.argv[1] if len(sys.argv) > 1 else 'bench/workloads/gen/c910_build/obj_vtr/Vtop.vdb.json')
