"""Live slang elaboration tests; run with the pinned exporter Python environment."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
EXPORT = ROOT / 'tools/vdb/export.py'


class ExportTests(unittest.TestCase):
    def export(self, text, success=True):
        with tempfile.TemporaryDirectory() as td:
            source = Path(td) / 'test.sv'
            output = Path(td) / 'test.json'
            source.write_text(text)
            p = subprocess.run([sys.executable, str(EXPORT), '--top', 'top', '-o', str(output), str(source)],
                               capture_output=True, text=True, cwd=ROOT)
            self.assertEqual(p.returncode == 0, success, p.stderr)
            if success:
                doc = json.loads(output.read_text())
                self.assertEqual(doc['sources'][0]['sha256'], hashlib.sha256(source.read_bytes()).hexdigest())
                return doc
            self.assertFalse(output.exists())

    def test_parameterized_hierarchy_and_connectivity(self):
        d = self.export('''module stage #(parameter W=3)(input logic [W-1:0] d, output logic [W-1:0] q);
          assign q=d; endmodule
          module top(input logic [4:0] a, output logic [4:0] b);
          stage #(.W(5)) u(.d(a),.q(b)); endmodule''')
        self.assertEqual(d['symbols']['top.u.W']['value'], '5')
        self.assertEqual(d['symbols']['top.u.q']['type']['width'], 5)
        self.assertEqual(d['instances'][1]['definition'], 'stage')
        self.assertEqual(len(d['connections']), 2)
        self.assertTrue(any(p['targets'] == ['top.b'] for p in d['processes']))
        self.assertTrue(all(p['source']['line'] > 0 for p in d['processes']))

    def test_invalid_rtl_is_rejected(self):
        self.export('module top; assign missing = not_defined; endmodule', success=False)

    def test_unsupported_statements_keep_driver_identity(self):
        d = self.export('''module top(input logic [1:0] a, output logic q);
          always_comb case (a) 0: q=0; default: q=1; endcase endmodule''')
        self.assertEqual(d['processes'][0]['targets'], ['top.q'])
        self.assertEqual(d['processes'][0]['body']['kind'], 'Unsupported')
        self.assertIn('Case', d['processes'][0]['body']['reason'])
        self.assertEqual(d['processes'][0]['reads'], ['top.a'])

    def test_explicit_ownership_ports_and_feedback(self):
        d = self.export("""module unit1(input logic a, output logic q); assign q=a; endmodule
          module top(input logic clk); logic state;
          always_ff @(posedge clk) state <= ~state;
          for(genvar i=0;i<2;i++) begin: g
            unit1 u(.a(state),.q());
          end endmodule""")
        self.assertEqual(d['version'], 2)
        self.assertEqual(d['instances'][0]['ports'][0]['direction'], 'In')
        self.assertTrue(all(i['parent'] == 'top' for i in d['instances'][1:]))
        self.assertTrue(all(s['owner'] in [i['path'] for i in d['instances']] for s in d['symbols'].values()))
        p = next(p for p in d['processes'] if p['mode'] == 'seq')
        self.assertEqual(p['reads'], ['top.clk', 'top.state'])
        self.assertEqual(p['owner'], 'top')
        self.assertEqual(len(d['instances'][1]['ports']), 2)  # includes unconnected q

    def test_committed_fixtures_match_live_elaboration(self):
        for name in ('pipeline', 'semantics', 'netlist', 'edge_cases'):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as td:
                out = Path(td)/'out.json'
                p = subprocess.run([sys.executable, str(EXPORT), '--top', 'top', '-o', str(out),
                                    f'crates/vtr-vdb/tests/rtl/{name}.sv'], capture_output=True, text=True, cwd=ROOT)
                self.assertEqual(p.returncode, 0, p.stderr)
                expected = json.loads((ROOT/f'crates/vtr-vdb/tests/fixtures/{name}.vdb.json').read_text())
                self.assertEqual(json.loads(out.read_text()), expected)
                self.assertEqual([f['path'] for f in expected['source_index']['files']],
                                 [f'crates/vtr-vdb/tests/rtl/{name}.sv'])

    def export_index(self):
        # The design the C++ verilator_vdb_index tests use; both producers must agree on it.
        fixtures = ROOT / 'ext/verilator/src/vdb_index/tests/fixtures'
        with tempfile.TemporaryDirectory() as td:
            shutil.copy(fixtures / 'design.sv', td)
            shutil.copytree(fixtures / 'include', Path(td) / 'include')
            out = Path(td) / 'design.vdb.json'
            p = subprocess.run([sys.executable, str(EXPORT), '--top', 'top', '-I', 'include', '-o', str(out),
                                'design.sv'], capture_output=True, text=True, cwd=td)
            self.assertEqual(p.returncode, 0, p.stderr)
            return json.loads(out.read_text())

    def test_source_index_is_not_part_of_the_identity(self):
        doc = self.export_index()
        hashed = {k: v for k, v in doc.items() if k not in ('design_id', 'source_index')}
        self.assertEqual(doc['design_id'], hashlib.sha256(json.dumps(hashed, sort_keys=True).encode()).hexdigest())
        index = doc['source_index']
        self.assertTrue(index['producer'].startswith('slang '))
        self.assertEqual(index['classes'], ['keyword', 'comment', 'number', 'string', 'operator', 'macro', 'variable',
                                            'parameter', 'enumMember', 'type', 'module', 'interface', 'package',
                                            'instance', 'function', 'property', 'namespace'])
        self.assertEqual(index['modifiers'], ['declaration', 'input', 'output', 'inout', 'ref', 'clock', 'readonly',
                                              'defaultLibrary', 'argument'])
        for f in index['files']:
            self.assertEqual(len(f['tokens']) % 5, 0)
            self.assertEqual(len(f['declarations']) % 4, 0)
            self.assertLess(max(f['declarations'][::4], default=-1), len(f['tokens']) // 5)

    def test_source_index_classifies_the_reference_design(self):
        # Mirrors ext/verilator/src/vdb_index/tests/IndexTests.cpp on the same design.
        index = self.export_index()['source_index']
        cls = {name: i for i, name in enumerate(index['classes'])}
        mod = {name: 1 << i for i, name in enumerate(index['modifiers'])}
        paths = [f['path'] for f in index['files']]
        self.assertEqual(sorted(paths), ['design.sv', 'include/defs.svh'])
        design = paths.index('design.sv')

        def tokens(path):
            f = index['files'][paths.index(path)]
            groups = [f['tokens'][i:i + 5] for i in range(0, len(f['tokens']), 5)]
            declared = {f['declarations'][i]: f['declarations'][i + 1:i + 4]
                        for i in range(0, len(f['declarations']), 4)}
            return {(line, column): dict(length=length, cls=c, mods=m, declaration=declared.get(i))
                    for i, (line, column, length, c, m) in enumerate(groups)}
        at = tokens('design.sv')
        defs = tokens('include/defs.svh')

        # Declarations: module, ports with directions, the clock heuristic.
        self.assertEqual((at[28, 8]['cls'], at[28, 8]['mods']), (cls['module'], mod['declaration']))
        self.assertEqual((at[17, 15]['cls'], at[17, 15]['mods']),
                         (cls['variable'], mod['declaration'] | mod['input'] | mod['clock']))  # lane's clk
        self.assertEqual(at[17, 32]['mods'], mod['declaration'] | mod['input'])  # rst_n is also read as data
        self.assertEqual((at[17, 83]['mods'], at[17, 83]['length']), (mod['declaration'] | mod['output'], 1))
        # References carry the declaration they denote.
        self.assertEqual((at[24, 38]['cls'], at[24, 38]['declaration']), (cls['function'], [design, 5, 34]))
        self.assertEqual((at[29, 3]['cls'], at[29, 3]['declaration']), (cls['package'], [design, 2, 9]))
        self.assertEqual((at[29, 8]['cls'], at[29, 8]['declaration']), (cls['type'], [design, 4, 60]))
        self.assertEqual((at[33, 11]['cls'], at[33, 11]['declaration']), (cls['parameter'], [design, 16, 29]))
        self.assertEqual((at[33, 61]['cls'], at[33, 61]['mods'], at[33, 61]['declaration']),
                         (cls['variable'], mod['input'], [design, 17, 59]))
        self.assertEqual((at[36, 27]['cls'], at[36, 27]['declaration']), (cls['instance'], [design, 33, 27]))
        self.assertEqual((at[36, 34]['cls'], at[36, 34]['mods'], at[36, 34]['declaration']),
                         (cls['variable'], mod['output'], [design, 17, 83]))
        self.assertEqual((at[36, 46]['cls'], at[36, 46]['declaration']), (cls['property'], [design, 4, 52]))
        self.assertEqual((at[33, 3]['cls'], at[33, 3]['declaration']), (cls['module'], [design, 16, 8]))
        self.assertEqual(at[30, 3]['cls'], cls['interface'])
        self.assertEqual((at[19, 3]['cls'], at[19, 3]['declaration']), (cls['type'], [design, 3, 40]))
        self.assertEqual(at[18, 10]['cls'], cls['package'])
        # Lexical classes: keywords, comments, numbers, macros, include files.
        self.assertEqual(at[28, 1]['cls'], cls['keyword'])
        self.assertEqual((at[29, 20]['cls'], at[29, 20]['length']), (cls['comment'], 15))
        self.assertEqual(at[31, 3]['cls'], cls['comment'])
        self.assertEqual((at[32, 1]['cls'], at[32, 1]['length']), (cls['comment'], 15))
        self.assertEqual(at[33, 13]['cls'], cls['number'])
        self.assertEqual((at[36, 54]['cls'], at[36, 54]['length']), (cls['macro'], 6))
        self.assertEqual(at[6, 16]['cls'], cls['macro'])
        self.assertEqual((at[1, 1]['cls'], at[1, 10]['cls']), (cls['macro'], cls['string']))
        self.assertEqual([defs[1, 1]['cls'], defs[2, 1]['cls'], defs[2, 9]['cls'], defs[2, 15]['cls']],
                         [cls['comment'], cls['macro'], cls['macro'], cls['number']])
        self.assertNotIn((3, 29), at)  # IDLE: enum members are not resolved, so they carry no token
        # Per-instance uninstantiated generate blocks and the definitions map.
        self.assertEqual(index['inactive'], {'top.u_fast': [[design, 22, 12, 25, 6]],
                                             'top.u_slow': [[design, 20, 13, 22, 6]]})
        self.assertEqual(index['definitions'], {'lane': [design, 16, 8], 'bus_if': [design, 10, 11],
                                                'pkg': [design, 2, 9], 'top': [design, 28, 8]})


if __name__ == '__main__':
    unittest.main()
