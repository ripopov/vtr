#!/usr/bin/env python3
"""Verify native elaboration and real VTR recordings with the RTL VDB tools."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--verilator', type=Path, required=True)
    parser.add_argument('--out', type=Path, default=ROOT/'bench/build/verilator-vdb')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    lib = out/'lib'
    lib.mkdir(exist_ok=True)
    shutil.copy2(ROOT/'target/release/libvtr.a', lib/'libvtr.a')
    env = dict(os.environ, VTR_INCLUDE=str(ROOT/'crates/vtr-capi/include'), VTR_LIBDIR=str(lib))
    cli = ROOT/'target/debug/vtr-vdb'

    def query(command, db, trace, *options, success=True):
        result = subprocess.run([str(cli), command, str(db), str(trace), *options],
                                text=True, capture_output=True)
        assert (result.returncode == 0) == success, result.stdout + result.stderr
        return result.stdout + result.stderr

    for name in ('pipeline', 'semantics', 'netlist', 'edge_cases', 'operators'):
        obj = out/name
        obj.mkdir(exist_ok=True)
        with (obj/'build.log').open('w') as log:
            subprocess.run([str(args.verilator.resolve()), '--cc', '--exe', '--build', '-j', '2',
                            '--top-module', 'top', '--prefix', 'Vtop', '--Mdir', str(obj),
                            '--trace-vtr', '-Wno-fatal', '-CFLAGS', '-DVDB_'+name.upper(),
                            str(HERE/'operators.sv' if name == 'operators' else ROOT/f'crates/vtr-vdb/tests/rtl/{name}.sv'), str(HERE/'main.cpp'), '-o', 'sim'],
                           env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
        recording = obj/'trace.vtr'
        subprocess.run([str(obj/'sim'), str(recording)], check=True)
        dbfile = obj/'trace.vdb.json'
        db = json.loads(dbfile.read_text())
        assert db['design_id'] == json.loads((obj/'Vtop.vdb.json').read_text())['design_id']
        assert len(db['design_id']) == 64
        assert db['trace_binding']['signals']['top.a'] == 'TOP.top.a'
        assert db['symbols']['top.a']['source']['file'].endswith(name+'.sv')
        assert db['symbols']['top.a']['source']['line'] > 0
        assert db['symbols']['top.a']['source']['column'] > 0
        result = query('check', dbfile, recording)
        assert 'structural match only' not in result
        for instance in db['instances']:
            svg = obj/(instance['path']+'.svg')
            query('netlist', dbfile, recording, instance['path'], '--time', '26', '--output', str(svg))
            ET.parse(svg)
        if name == 'pipeline':
            tree = query('trace', dbfile, recording, 'top.q', '--time', '26', '--depth', '14')
            (obj/'trace.txt').write_text(tree)
            for text in ('top.q = 00000011 @ 26', 'hold at 15', 'PosEdge top.u0.clk @ 5, event #1',
                         'top.a = 00000011 @ 5- (pre-event)', 'pipeline.sv:5:'):
                assert text in tree, tree
            assert all(text not in tree for text in ('mismatch', 'ambiguous', 'unsupported')), tree
            reset = query('trace', dbfile, recording, 'top.u0.q', '--time', '3')
            assert 'NegEdge top.u0.rst_n @ 2' in reset, reset
            changed = dict(db, design_id='0'*64)
            bad = obj/'wrong.vdb.json'
            bad.write_text(json.dumps(changed))
            assert 'design.vdb_id differs' in query('check', bad, recording, success=False)
            assert 'conflicts' in query('check', dbfile, recording, '--prefix', 'wrong', success=False)
            subprocess.run([str(obj/'sim'), str(obj/'named.vtr'), 'soc'], check=True)
            query('check', obj/'named.vdb.json', obj/'named.vtr')
            subprocess.run([str(obj/'sim'), str(obj/'empty.vtr'), ''], check=True)
            query('check', obj/'empty.vdb.json', obj/'empty.vtr')
            partial = obj/'partial'
            with (obj/'partial.log').open('w') as log:
                subprocess.run([str(args.verilator.resolve()), '--cc', '--exe', '--build', '-j', '2',
                                '--top-module', 'top', '--prefix', 'Vtop', '--Mdir', str(partial),
                                '--trace-vtr', '--trace-depth', '1', '-CFLAGS', '-DVDB_PIPELINE',
                                str(ROOT/'crates/vtr-vdb/tests/rtl/pipeline.sv'), str(HERE/'main.cpp'), '-o', 'sim'],
                               env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
            subprocess.run([str(partial/'sim'), str(partial/'trace.vtr')], check=True)
            missing = query('check', partial/'trace.vdb.json', partial/'trace.vtr', success=False)
            assert 'not recorded in VDB binding' in missing, missing
            query('netlist', partial/'trace.vdb.json', partial/'trace.vtr', 'top',
                  '--time', '26', '--output', str(partial/'top.svg'))
            assert 'unavailable' in (partial/'top.svg').read_text()

        elif name == 'semantics':
            for signal, value in (('y', '00001001'), ('q', '00001001'), ('ordered', '00001001'), ('z', '00000000')):
                tree = query('trace', dbfile, recording, 'top.'+signal, '--time', '26', '--depth', '10')
                assert f'top.{signal} = {value} @ 26' in tree, tree
                assert all(text not in tree for text in ('mismatch', 'ambiguous', 'unsupported')), tree
        elif name == 'netlist':
            paths = {i['path'] for i in db['instances']}
            assert {'top.group0.u', 'top.lanes[0].u', 'top.lanes[1].u'} <= paths, paths
            assert db['symbols']['top.group0.u.q']['type']['width'] == 8
            assert db['symbols']['top.lanes[0].u.q']['type']['width'] == 4
            assert db['symbols']['top.wide']['type']['width'] == 96
            assert '11011110101011011011111011101111' in (obj/'top.svg').read_text()
        elif name == 'edge_cases':
            tree = query('trace', dbfile, recording, 'top.q', '--time', '26')
            assert 'unsupported' in tree, tree
            assert any(p['reads'] == ['top.a', 'top.sel', 'top.state'] for p in db['processes'])
        else:
            assert db['symbols']['top.IDLE']['kind'] == 'EnumValue'
            assert db['symbols']['top.ACTIVE']['value'].endswith("'h7")
            outputs = [p['symbol'] for p in db['instances'][0]['ports'] if p['direction'] == 'Out']
            for symbol in outputs:
                for time in ('6', '26'):
                    tree = query('trace', dbfile, recording, symbol, '--time', time, '--depth', '10')
                    (obj/(symbol+'.'+time+'.txt')).write_text(tree)
                    assert all(text not in tree for text in ('mismatch', 'ambiguous', 'unsupported')), tree
        print(name+': source mapping, attachment, module SVGs and driver semantics verified')


if __name__ == '__main__':
    main()
