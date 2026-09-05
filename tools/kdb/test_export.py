"""Live slang elaboration tests; run with the pinned exporter Python environment."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
EXPORT = ROOT / 'tools/kdb/export.py'


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
                                    f'crates/vtr-kdb/tests/rtl/{name}.sv'], capture_output=True, text=True, cwd=ROOT)
                self.assertEqual(p.returncode, 0, p.stderr)
                expected = json.loads((ROOT/f'crates/vtr-kdb/tests/fixtures/{name}.kdb.json').read_text())
                self.assertEqual(json.loads(out.read_text()), expected)


if __name__ == '__main__':
    unittest.main()
