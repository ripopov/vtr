#!/usr/bin/env python3
"""Build and verify timestamped HDL log capture, including abort-time finalization."""
import argparse
import os
from pathlib import Path
import re
import resource
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent


def records(cli, trace):
    text = subprocess.check_output([str(cli), 'log', str(trace)], text=True)
    headers = list(re.finditer(r'(?m)^(\d+) (trace|debug|info|warn|error|fatal) +simulation_log: ', text))
    return [(int(m[1]), m[2], text[m.end():headers[i + 1].start() if i + 1 < len(headers) else len(text)].removesuffix('\n'))
            for i, m in enumerate(headers)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--verilator', type=Path, required=True)
    parser.add_argument('--out', type=Path, default=ROOT / 'bench/build/verilator-logs')
    parser.add_argument('--update-examples', action='store_true')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    env = dict(os.environ, VTR_INCLUDE=str(ROOT / 'crates/vtr-capi/include'), VTR_LIBDIR=str(ROOT / 'target/release'))
    cli = ROOT / 'target/release/vtr'
    for threads in [1, 2]:
        obj = out / f'obj_{threads}'
        with (out / f'build_{threads}.log').open('w') as log:
            subprocess.run([str(args.verilator.resolve()), '--cc', '--exe', '--build', '-j', '4',
                            '--trace-vtr', '--assert', '--threads', str(threads), '--top-module', 'top',
                            '--Mdir', str(obj), str(HERE / 'top.sv'), str(HERE / 'main.cpp'), '-o', 'sim'],
                           env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
        for mode in ['normal', 'error', 'fatal', 'finish', 'bytes', 'runtime', 'repeat', 'context']:
            run = out / f'{mode}_{threads}'
            run.mkdir(exist_ok=True)
            result = subprocess.run([str(obj / 'sim'), '+' + mode], cwd=run, capture_output=True, text=True, errors="replace", timeout=30)
            (run / 'console.txt').write_text(result.stdout + result.stderr)
            assert (result.returncode != 0) == (mode in ['error', 'fatal']), (mode, result)
            rows = records(cli, run / 'logs.vtr')
            assert rows[0] == (0, 'info', 'boot complete\n'), rows
            assert rows[1][0:2] == (0, 'info') and 'configuration ready' in rows[1][2], rows
            assert rows[2][0:2] == (0, 'warn') and 'retry pending' in rows[2][2], rows
            assert rows[3:6] == [(0, 'info', 'file message\n'), (0, 'info', 'fragment '), (0, 'info', 'complete\n')], rows
            assert (run / 'messages.txt').read_text() == 'file message\n'
            assert (1, 'info', 'cycle at 1\n') in rows and (3, 'info', 'settled at 3\n') in rows, rows
            if mode in ['normal', 'context']:
                assert len(rows) == 15, rows
                assert rows[-1] == (7, 'info', 'simulation complete\n'), rows
            elif mode == 'error':
                assert any(t == 5 and level == 'error' and 'response rejected' in text for t, level, text in rows), rows
            elif mode == 'fatal':
                assert any(t == 5 and level == 'fatal' and 'watchdog expired' in text for t, level, text in rows), rows
            elif mode == 'repeat':
                assert any(t == 3 and level == 'warn' and 'previous dump' in text for t, level, text in rows), rows
            elif mode == 'runtime':
                assert any(t == 5 and level == 'info' and 'Time scale' in text for t, level, text in rows), rows
            elif mode == 'bytes':
                assert (5, 'info', '[non-UTF-8 bytes] 72617720ff0a') in rows, rows
            else:
                assert any(t == 5 and level == 'info' and 'Verilog $finish' in text for t, level, text in rows), rows
            if args.update_examples and threads == 1:
                shutil.copyfile(run / 'logs.vtr', ROOT / f'ext/surfer/examples/verilator/logs_{mode}.vtr')
            print(f'{mode}, {threads} threads: {len(rows)} records; timestamps, severity and output verified')


if __name__ == '__main__':
    main()
