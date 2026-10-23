#!/usr/bin/env python3
"""Generate actual Verilator log fixtures in inline and threaded execution modes."""
import argparse
import os
from pathlib import Path
import subprocess
import shutil

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
SCENARIOS = ('normal', 'error', 'fatal', 'finish', 'bytes', 'runtime', 'repeat')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--verilator', type=Path, required=True)
    parser.add_argument('--out', type=Path, default=ROOT/'build/verilator-logs')
    parser.add_argument('--fixtures', type=Path, default=ROOT/'vtr/tests/fixtures/verilator')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    args.fixtures.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, VTR_INCLUDE=str(ROOT/'vtr-cpp'), VTR_LIBDIR=str(ROOT/'build/cpp'))
    (out/'obj/sim').unlink(missing_ok=True)
    with (out/'build.log').open('w') as log:
        subprocess.run([str(args.verilator.resolve()), '--cc', '--exe', '--build', '-j', '2',
                        '--top-module', 'top', '--prefix', 'Vtop', '--Mdir', str(out/'obj'),
                        '--trace-vtr', '--assert', '-CFLAGS', f'-I{ROOT/"vtr-cpp"}',
                        '-LDFLAGS', '-lvtr -llz4 -lzstd -lz', str(HERE/'top.sv'),
                        str(HERE/'main.cpp'), '-o', 'sim'], env=env,
                       stdout=log, stderr=subprocess.STDOUT, check=True)
    traces = out/'traces'
    traces.mkdir(exist_ok=True)
    for execution in ('inline', 'threaded'):
        for mode, name in enumerate(SCENARIOS):
            path = traces/f'logs_{name}_{execution}.vtr'
            with (out/f'{name}_{execution}.log').open('wb') as log:
                result = subprocess.run([str(out/'obj/sim'), str(path), str(mode), execution],
                                        env=env, stdout=log, stderr=subprocess.STDOUT)
            assert result.returncode == (1 if name == 'fatal' else 0), (name, execution, result.returncode)
            shutil.copy2(path, args.fixtures/path.name)
    print('Generated all 14 Verilator log fixtures.')


if __name__ == '__main__':
    main()
