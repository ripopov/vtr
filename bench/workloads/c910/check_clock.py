#!/usr/bin/env python3
"""Check the declared core clock of a C910 VTR capture (docs/vtr_clocks.html).

The capture must hold one stretch of the clock TX.core0.clk, period 2 (200 ps
in 100 ps units), starting on the first rising edge of the recorded top.clk
waveform, with as many edges as that waveform has rising edges.

  check_clock.py <vtr-cli> <c910_coremark.vtr>
"""
import re
import subprocess
import sys


def main():
    cli, trace = sys.argv[1:3]
    out = subprocess.check_output([cli, 'clocks', trace], text=True).splitlines()
    assert re.match(r'clock 0 TX\.core0\.clk: \d+ edges, 1 stretches, stopped 0, running at close$', out[0]), out
    m = re.match(r'\s+\[(\d+) \.\. (\d+)\] period (\d+), (\d+) edges from cycle 0$', out[1])
    assert m and len(out) == 2, out
    begin, end, period, edges = map(int, m.groups())
    rising, prev = [], None
    with subprocess.Popen([cli, 'changes', trace, 'TOP.top.clk'], stdout=subprocess.PIPE, text=True) as p:
        for line in p.stdout:
            t, v = line.split('\t')
            v = v.strip()
            if v == '1' and prev == '0':
                rising.append(int(t))
            prev = v
    assert period == 2, period
    assert rising[0] == begin, (rising[0], begin)
    assert rising[-1] == end and len(rising) == edges, (rising[-1], end, len(rising), edges)
    print(f'clk: one stretch of period {period} from {begin} to {end}, {edges} edges = rising edges of the waveform')


if __name__ == '__main__':
    main()
