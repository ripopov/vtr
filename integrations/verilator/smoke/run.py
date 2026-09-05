#!/usr/bin/env python3
"""Build the same model with VTR/FST, then check recorded values and hierarchy."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--verilator', type=Path, required=True)
    parser.add_argument('--out', type=Path, default=ROOT/'bench/build/verilator-smoke')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    lib = out/'lib'
    lib.mkdir(exist_ok=True)
    shutil.copy2(ROOT/'target/release/libvtr.a', lib/'libvtr.a')
    env = dict(os.environ, VTR_INCLUDE=str(ROOT/'crates/vtr-capi/include'), VTR_LIBDIR=str(lib))
    cli = ROOT/'target/release/vtr'
    for mode in ('vtr', 'fst'):
        obj = out/f'obj_{mode}'
        flags = []
        if mode == 'fst':
            for option, query in [('-CFLAGS', '--cflags'), ('-LDFLAGS', '--libs')]:
                value = subprocess.check_output(['pkg-config', query, 'liblz4'], text=True).strip()
                if value:
                    flags.extend([option, value])
        with (out/f'build_{mode}.log').open('w') as log:
            subprocess.run([str(args.verilator.resolve()), '--cc', '--exe', '--build', '-j', '2',
                            '--top-module', 'top', '--prefix', 'Vtop', '--Mdir', str(obj),
                            f'--trace-{mode}', *flags, str(HERE/'top.sv'), str(HERE/'main.cpp'), '-o', 'sim'],
                           env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
        trace = out/f'trace.{mode}'
        subprocess.run([str(obj/'sim'), str(trace)], check=True)
        if mode == 'fst':
            converted = out/'fst.vtr'
            subprocess.run([str(cli), 'convert', str(trace), str(converted)], check=True)
            trace = converted
        for t in range(8):
            expected = 0 if t == 0 else (t+3 if t % 2 else t+2)
            for signal in ('TOP.top.q [7:0]', 'TOP.top.u.q [7:0]'):
                actual = subprocess.check_output([str(cli), 'value', str(trace), signal, str(t)], text=True).strip()
                assert actual == f'{expected:08b}', (mode, signal, t, actual, expected)
    print('VTR and FST agree with all 16 expected output/alias samples each.')


if __name__ == '__main__':
    main()
