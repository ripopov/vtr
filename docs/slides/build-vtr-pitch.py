#!/usr/bin/env python3
"""Generate docs/vtr-pitch.html: the pitch slides for the AI skill, VTR/VDB and Volna.

Slide 2 is two columns: optimization pushes the design down to RTL, the only
executable specification as detailed as silicon; agents lift its simulation back
up, each lifted level facing the design level it came from.


Slide 3 is VTR itself: the ideas it takes from FST (waveforms) and FTR
(transactions), packed into one compressed file, and the measured result against
the best FST or FTR variant per metric, read from bench/results/latest/results.json.

Slide 4 is Volna: the best ideas of five viewers (GTKWave, Surfer, SCViewer,
Perfetto, Konata), each feeding the panel it inspired in one Rust core, which
runs as a native GPUI app, a VS Code plugin and a WASM page for CI.

"Level up your traces" is drawn as a staircase: L0 your RTL and its simulation,
L1 the AI skill and the VTR monitor it writes, L2 VTR transactions named by VDB,
L3 what you get in Volna (pipeline, sequence and bandwidth). Four equal
steps carry equally sized, abstract diagrams. The
slide is a fixed 1280x720 canvas, so the diagram is drawn at 1:1 pixel scale.

The page embeds subsets of Volna's own fonts (IBM Plex Sans, Lilex) so it
renders identically offline and in CI. Subsetting needs fontTools and brotli:

    python3 -m pip install fonttools brotli
    python3 docs/slides/build-vtr-pitch.py
    node --test docs/tests/vtr-pitch.test.mjs
"""
import base64
import html
import io
import json
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parents[2]
FONTS = ROOT / 'volna/volna-core/assets/fonts'
OUT = ROOT / 'docs/vtr-pitch.html'
RESULTS = ROOT / 'bench/results/latest/results.json'

TITLE = 'Level up your traces'
SUBTITLE = 'See your hardware run at the level you designed it.'
V_TITLE = 'Optimized down. Lifted back up.'
V_SUBTITLE = 'Optimization pushes every design down to RTL, the only executable spec as detailed as silicon.'
F_TITLE = 'Two formats. One modern file.'
F_SUBTITLE = 'VTR packs the best ideas of FST waveforms and FTR transactions into one compressed format.'
W_TITLE = 'Five viewers. One modern framework.'
W_SUBTITLE = 'Volna packs the best ideas of five viewers into one Rust core that runs everywhere.'
SENTENCES = (
    'An AI skill reads your RTL and generates monitors with automated checks.',
    'VTR records transactions and runtime links; VDB gives them design meaning.',
    'Volna makes stalls and bottlenecks visible in an interactive view of your design.',
)


def esc(s):
    return html.escape(str(s), quote=True)


class Svg:
    """A diagram drawn in slide pixels; colours come from the slide's tokens."""

    def __init__(self, key, w, h, label):
        self.key, self.w, self.h, self.label, self.out = key, w, h, label, []

    def add(self, s):
        self.out.append(s)

    def rect(self, x, y, w, h, cls, r=0, box=True):
        self.add(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" class="{cls}"'
                 f'{" data-box" if box else ""}/>')

    def text(self, x, y, s, size=13, cls='c-ink', mono=False, weight=400, anchor='start', track=0):
        ff = 'ff-mono' if mono else 'ff-sans'
        wt = ' w6' if weight >= 600 else ''
        ls = f' letter-spacing="{track}"' if track else ''
        self.add(f'<text x="{x}" y="{y}" font-size="{size}" class="{ff}{wt} {cls}"'
                 f' text-anchor="{anchor}"{ls}>{esc(s)}</text>')

    def path(self, d, cls, arrow=None, dash=None):
        m = f' marker-end="url(#{self.key}-{arrow})"' if arrow else ''
        da = f' stroke-dasharray="{dash}"' if dash else ''
        self.add(f'<path d="{d}" class="{cls}" fill="none"{m}{da}/>')

    def arrow(self, x1, y1, x2, y2, tone='mut'):
        self.path(f'M{x1} {y1} L{x2} {y2}', f'ln-{tone}', arrow=tone)

    def wave(self, x0, y, cw, bits, amp, cls='ln-sig'):
        """A 1-bit waveform; bits is one character per cycle."""
        hi, lo = y - amp / 2, y + amp / 2
        level = bits[0] == '1'
        d = f'M{x0} {hi if level else lo}'
        for i, b in enumerate(bits[1:], 1):
            if (b == '1') != level:
                level = b == '1'
                d += f' H{x0 + i * cw} V{hi if level else lo}'
        d += f' H{x0 + len(bits) * cw}'
        self.path(d, cls)

    def clock(self, x0, y, cw, cycles, amp, cls='ln-sig'):
        hi, lo = y - amp / 2, y + amp / 2
        d = f'M{x0} {lo}'
        for i in range(cycles):
            x = x0 + i * cw
            d += f' V{hi} H{x + cw / 2} V{lo} H{x + cw}'
        self.path(d, cls)

    def bus(self, x0, y, cw, cycles, segments, amp, size=10, cls='ln-sig'):
        """A multi-bit bus: segments are (start, end, value) in cycles; gaps idle."""
        hi, lo, k = y - amp / 2, y + amp / 2, min(3, cw / 4)
        idle, c = [], 0
        for a, b, _ in segments:
            if a > c:
                idle.append((c, a))
            c = b
        if c < cycles:
            idle.append((c, cycles))
        for a, b in idle:
            self.path(f'M{x0 + a * cw + (k if a else 0)} {y} H{x0 + b * cw - (k if b < cycles else 0)}', cls)
        for a, b, v in segments:
            xa, xb = x0 + a * cw, x0 + b * cw
            self.path(f'M{xa} {y} L{xa + k} {hi} H{xb - k} L{xb} {y} L{xb - k} {lo} H{xa + k} Z', cls + ' bus')
            self.text((xa + xb) / 2, y + size * 0.36, v, size, 'c-ink', mono=True, anchor='middle')

    def mark(self, x, y, mark, size, tone):
        """A status mark whose baseline is y: a drawn tick for pass, a glyph otherwise."""
        if mark == '✓':
            u = size / 13
            self.path(f'M{x + 1 * u:.1f} {y - 4.5 * u:.1f} L{x + 4.5 * u:.1f} {y - 1 * u:.1f} L{x + 11 * u:.1f} {y - 8.5 * u:.1f}',
                      tone.replace('c-', 'ln-'))
        else:
            self.text(x, y, mark, size, tone)

    def code(self, x, y, parts, size=11):
        """One line of source code; parts are (text, colour class) runs."""
        runs = ''.join(f'<tspan class="{c}">{esc(t)}</tspan>' for t, c in parts)
        self.add(f'<text x="{x}" y="{y}" font-size="{size}" class="ff-mono">{runs}</text>')

    def meter(self, x, y, level, of=4):
        """A small signal-strength style level meter whose bottom sits on y."""
        for i in range(of):
            h = 4 + 2 * i
            self.rect(x + i * 5, y - h, 3, h, 'f-acc' if i < level else 'f-hair', r=1, box=False)

    def render(self, extra_defs=''):
        marks = ''.join(
            f'<marker id="{self.key}-{t}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7"'
            f' orient="auto-start-reverse"><path d="M1 1.5 L9 5 L1 8.5" class="mk-{t}" fill="none"/></marker>'
            for t in ('mut', 'acc', 'ok', 'warn', 'inv'))
        return (f'<svg class="dg" viewBox="0 0 {self.w} {self.h}" width="{self.w}" height="{self.h}"'
                f' role="img" aria-label="{esc(self.label)}"><defs>{marks}{extra_defs}</defs>'
                + ''.join(self.out) + '</svg>')



def staircase():
    s = Svg('p', 1136, 624,
            'Four equal steps rise from RTL to design insight. L0: top.sv RTL and simulation waveforms. '
            'L1: an AI skill generates a VTR monitor, checks it and refines it. '
            'L2: VTR records transactions and runtime links; VDB supplies design meaning. '
            'Requests connect the load, cache miss and DRAM read; data returns to the CPU. L3: Volna reveals stalls and bottlenecks through staggered pipeline stages, sequence and bandwidth views.')
    treads = (488, 436, 384, 332)
    grad = ('<linearGradient id="p-step" x1="0" y1="0" x2="0" y2="1">'
            '<stop offset="0" class="st-top"/><stop offset="1" class="st-bot"/></linearGradient>')
    for i, t in enumerate(treads):
        x = i * 284
        s.add(f'<rect x="{x}" y="{t}" width="284" height="{624 - t}" fill="url(#p-step)"/>')
        s.path(f'M{x} {t} H{x + 284}', 'ln-tread')
        if i:
            s.path(f'M{x} {treads[i - 1]} V{t}', 'ln-riser')

    def panel(i, title, note):
        x, y = i * 284 + 16, treads[i] - 192
        s.rect(x, y, 252, 176, 'f-panel', r=12)
        s.text(x + 20, y + 30, title, 17, weight=600)
        s.text(x + 20, y + 53, note, 12, 'c-mut')
        return x + 20, y

    # One compact abstraction per level, with the same outer dimensions.
    x, y = panel(0, 'RTL + simulation', 'Your design, at signal level')
    s.text(x, y + 78, 'top.sv', 11, 'c-mut', mono=True)
    for n, line in enumerate(('module top;', '  cpu u_cpu', '    (.*);', 'endmodule')):
        s.text(x, y + 98 + n * 16, line, 10, 'c-acc-ink' if n in (0, 3) else 'c-ink', mono=True)
    s.path(f'M{x + 112} {y + 72} V{y + 153}', 'ln-hair')
    for n, (name, bits) in enumerate((('clk', '01010101'), ('req', '00111100'), ('ack', '00000110'))):
        s.text(x + 126, y + 78 + 29 * n, name, 9, 'c-mut', mono=True)
        s.wave(x + 126, y + 89 + 29 * n, 10, bits, 8)

    x, y = panel(1, 'AI skill', 'Generate. Check. Refine.')
    s.rect(x, y + 78, 76, 46, 'f-chip', r=8)
    s.text(x + 38, y + 107, 'RTL', 14, anchor='middle', mono=True)
    s.arrow(x + 84, y + 101, x + 120, y + 101, 'acc')
    s.rect(x + 128, y + 78, 84, 46, 'f-acc-soft', r=8)
    s.text(x + 170, y + 107, 'Monitor', 14, 'c-acc-ink', anchor='middle')
    s.path(f'M{x + 170} {y + 132} V{y + 148} H{x + 38} V{y + 132}', 'ln-ok', arrow='ok')
    s.mark(x + 98, y + 144, '✓', 13, 'c-ok')

    x, y = panel(2, 'VTR + VDB', 'Runtime facts + design meaning')
    # Nested transactions, explicit request causality and the returning data.
    for n, (name, start, width, label, tone) in enumerate((
            ('CPU', 0, 142, 'load', 'f-acc'),
            ('L2', 22, 104, 'miss', 'f-acc'),
            ('DRAM', 48, 62, 'read', 'f-warn'))):
        ry = y + 72 + n * 32
        s.text(x, ry + 12, name, 11, 'c-mut', mono=True)
        s.rect(x + 58 + start, ry, width, 18, tone, r=4)
        s.text(x + 58 + start + width / 2, ry + 12, label, 10,
               'c-on-warn' if n == 2 else 'c-on-acc', mono=True, anchor='middle')
    s.path(f'M{x + 68} {y + 90} V{y + 97} H{x + 90} V{y + 104}', 'ln-acc', arrow='acc')
    s.path(f'M{x + 94} {y + 122} V{y + 129} H{x + 116} V{y + 136}', 'ln-acc', arrow='acc')
    s.path(f'M{x + 168} {y + 145} H{x + 207} V{y + 81} H{x + 200}', 'ln-mut', arrow='mut')

    x, y = panel(3, 'Volna', 'See stalls and bottlenecks')
    # Diagonal stage progression, with younger instructions held by a load miss.
    for row, (name, start, stages) in enumerate((('ld', 0, 'FDEMMMW'),
                                                ('add', 1, 'FDE--EW'),
                                                ('st', 2, 'FD--EMW'))):
        ry = y + 69 + row * 15
        s.text(x, ry + 10, name, 10, 'c-mut', mono=True)
        for offset, stage in enumerate(stages):
            col = start + offset
            stall = col in (4, 5)
            s.rect(x + 70 + col * 15, ry, 13, 12,
                   'f-warn' if stall else 'f-chip', r=2, box=False)
            s.text(x + 76.5 + col * 15, ry + 9, stage, 8,
                   'c-on-warn' if stall else 'c-ink', mono=True, anchor='middle')
    s.text(x, y + 135, 'Sequence', 10, 'c-mut')
    for col in range(3):
        sx = x + 78 + col * 60
        s.path(f'M{sx} {y + 119} V{y + 140}', 'ln-hair')
    s.arrow(x + 78, y + 124, x + 138, y + 124, 'acc')
    s.arrow(x + 138, y + 135, x + 198, y + 135, 'acc')
    s.text(x, y + 160, 'Bandwidth', 10, 'c-mut')
    for col, height in enumerate((12, 15, 14, 12, 4, 4, 18, 14, 12)):
        s.rect(x + 70 + col * 15, y + 163 - height, 13, height,
               'f-bw-stall' if col in (4, 5) else 'f-bw', r=2, box=False)

    return s.render(grad)


def down_up():
    """Slide 2: two columns. Optimization pushes the design down to RTL; agents lift its run back up."""
    s = Svg('v', 1136, 450,
            'Two columns. The left column is the design, pushed down by optimization through architecture, '
            'micro-architecture and interfaces to RTL, the only executable specification as detailed as silicon. '
            'At the bottom the RTL is simulated into waveforms, where debug starts today. The '
            'right column is the trace, lifted by agents level by level: waveforms, transactions, pipelines and '
            'causes, flows and performance. An AI skill lifts the waveforms, VTR and VDB record and name every '
            'level, and Volna shows the run back in the concept space where the design began. Each lifted level '
            'faces the design level it came from.')
    W, H, CX = 300, 72, 568
    left = (('Architecture', 'what it should do'), ('Micro-architecture', 'pipelines, queues'),
            ('Interfaces', 'protocols, handshakes'), ('RTL', 'the executable spec'))
    right = (('Flows, performance', 'bandwidth, latency'), ('Pipelines, causes', 'who waits on whom'),
             ('Transactions', 'requests, responses'), ('Waveforms', 'FST, FSDB, VCD'))
    row_y = lambda i: 38 + 100 * i
    lx = lambda i: 24
    rx = lambda i: 1136 - 24 - W

    s.text(0, 12, 'DESIGN ↓ PUSHED DOWN BY OPTIMIZATION', 11, 'c-mut', mono=True, track=1.2)
    s.text(1136, 12, 'TRACE ↑ LIFTED BY AGENTS', 11, 'c-acc-ink', mono=True, anchor='end', track=1.2)
    s.rect(0, 24, 1136, 100, 'f-band', r=14, box=False)
    s.text(CX, 58, 'CONCEPT SPACE', 11, 'c-acc-ink', mono=True, anchor='middle', track=1.2)
    s.text(CX, 80, 'where the design began,', 13, 'c-mut', anchor='middle')
    s.text(CX, 98, 'and where Volna shows the run', 13, 'c-ink', anchor='middle')

    # The arms: mirrored vertical arrows along the outer edges of the columns, down then up.
    s.arrow(lx(0) - 14, row_y(0) + H / 2, lx(3) - 14, row_y(3) + H / 2, 'mut')
    s.arrow(rx(3) + W + 14, row_y(3) + H / 2, rx(0) + W + 14, row_y(0) + H / 2, 'acc')

    def glyph(kind, x, y):
        """A 72x40 miniature of what the level looks like, drawn at (x, y)."""
        if kind == 'spec':
            s.rect(x + 16, y, 40, 40, 'f-chip', r=4, box=False)
            s.path(f'M{x + 24} {y + 10} H{x + 44}', 'ln-acc')
            for n in range(3):
                s.path(f'M{x + 24} {y + 19 + 7 * n} H{x + 48 - 6 * (n == 2)}', 'ln-mut')
        elif kind == 'uarch':
            for n in range(3):
                s.rect(x + 26 * n, y + 12, 18, 16, 'f-chip', r=3, box=False)
            s.path(f'M{x + 19} {y + 20} H{x + 25} M{x + 45} {y + 20} H{x + 51}', 'ln-mut')
        elif kind == 'iface':
            s.rect(x + 2, y + 6, 20, 28, 'f-chip', r=3, box=False)
            s.rect(x + 50, y + 6, 20, 28, 'f-chip', r=3, box=False)
            s.arrow(x + 26, y + 14, x + 46, y + 14, 'mut')
            s.arrow(x + 46, y + 26, x + 26, y + 26, 'mut')
        elif kind == 'rtl':
            for n, (w, tone) in enumerate(((44, 'ln-acc'), (56, 'ln-mut'), (38, 'ln-mut'), (30, 'ln-acc'))):
                s.path(f'M{x + 8 + 10 * (n in (1, 2))} {y + 6 + 9 * n} H{x + 8 + w}', tone)
        elif kind == 'perf':
            for n, v in enumerate((0.7, 0.8, 0.75, 0.15, 0.1, 0.95, 0.8)):
                h = round(36 * v)
                s.rect(x + 2 + 10 * n, y + 38 - h, 8, h, 'f-bw-stall' if n in (3, 4) else 'f-bw', r=1.5, box=False)
        elif kind == 'pipe':
            for r, (start, stall) in enumerate(((0, (3,)), (1, (3,)), (2, ()))):
                for c in range(4):
                    s.rect(x + 2 + 12 * (start + c), y + 4 + 12 * r, 10, 9,
                           'f-warn' if (start + c) in stall else 'f-chip', r=2, box=False)
        elif kind == 'txn':
            for n, (a, w, cls) in enumerate(((0, 64, 'f-acc'), (10, 38, 'f-acc2'), (22, 30, 'f-warn'))):
                s.rect(x + 4 + a, y + 4 + 13 * n, w, 9, cls, r=2, box=False)
        elif kind == 'wave':
            s.clock(x + 2, y + 8, 8, 8, 8)
            s.wave(x + 2, y + 21, 8, '00111000', 8)
            s.wave(x + 2, y + 34, 8, '00011100', 8)

    kinds = (('spec', 'perf'), ('uarch', 'pipe'), ('iface', 'txn'), ('rtl', 'wave'))
    for i in range(4):
        y = row_y(i)
        for (title, note), x, cls, kind in ((left[i], lx(i), 'f-panel', kinds[i][0]),
                                            (right[i], rx(i), 'f-lift', kinds[i][1])):
            s.rect(x, y, W, H, cls, r=12)
            s.text(x + 16, y + 31, title, 16, weight=600)
            s.text(x + 16, y + 52, note, 12, 'c-mut')
            glyph(kind, x + W - 88, y + 16)

    # What sits between the arms, bottom to top: the skill lifts, VTR and VDB keep, Volna shows.
    for i, (chip, note) in ((2, ('AI skill', 'monitors, checked on signals')),
                            (1, ('VTR · VDB', 'records and names every level'))):
        cy = row_y(i) + 30
        s.path(f'M{lx(i) + W + 8} {cy} H{CX - 72}', 'ln-hair', dash='3 4')
        s.path(f'M{CX + 72} {cy} H{rx(i) - 8}', 'ln-hair', dash='3 4')
        s.rect(CX - 64, cy - 15, 128, 30, 'f-acc-soft', r=15)
        s.text(CX, cy + 5, chip, 13, 'c-acc-ink', weight=600, anchor='middle')
        s.text(CX, cy + 36, note, 11, 'c-mut', mono=True, anchor='middle')
    s.path(f'M{lx(0) + W + 8} {row_y(0) + 36} H{CX - 128}', 'ln-hair', dash='3 4')
    s.path(f'M{CX + 128} {row_y(0) + 36} H{rx(0) - 8}', 'ln-hair', dash='3 4')

    # The vertex: RTL is simulated into waveforms, where today's debug starts.
    cy = row_y(3) + 36
    s.arrow(lx(3) + W + 10, cy, rx(3) - 10, cy, 'mut')
    s.text(CX, cy - 8, 'simulate', 11, 'c-mut', mono=True, anchor='middle')
    s.text(lx(3) + W / 2, row_y(3) + H + 26, 'as detailed as silicon', 11, 'c-mut', mono=True, anchor='middle')
    s.text(rx(3) + W / 2, row_y(3) + H + 26, 'today, debug starts here', 11, 'c-warn', mono=True, anchor='middle')
    return s.render()


def benchmarks():
    """The measured rows of slide 3: VTR against the best FST or FTR variant for each metric."""
    d = json.loads(RESULTS.read_text())
    rtl = next(r for r in d['rtl'] if r['workload'] == 'c910_coremark')
    tlm = next(r for r in d['tx'] if r['workload'] == 'tlm_1m')
    fst = [rtl['writers'][k] for k in ('fstapi-lz4', 'fstapi-zlib', 'fstcpp-lz4')]
    vtr, rd = rtl['writers']['vtr-rust'], rtl['readers']
    ftr = [tlm['writers'][k] for k in ('ftr-lz4', 'ftr-raw')]
    mib = lambda b: f'{b / 2**20:.0f} MiB' if b >= 100 * 2**20 else f'{b / 2**20:.1f} MiB'
    sec = lambda t: f'{t:.1f} s' if t >= 1 else f'{t:.2f} s'
    ms = lambda t: f'{t * 1e3:.1f} ms'
    info = rtl['info']
    return (
        ('Waveforms vs FST',
         f'openC910 CoreMark · {info["signals"]:,} signals · {info["changes"] / 1e6:.0f} M changes',
         (('File size', 'FST', min(w['bytes'] for w in fst), vtr['bytes'], mib, 'smaller'),
          ('Write time', 'FST', min(w['wall_s'] for w in fst), vtr['wall_s'], sec, 'faster'),
          ('Read one signal', 'FST', min(rd['vs_fstapi_zlib']['load_1']['fst_wellen'], rd['fstapi_reader_zlib']['load_1_s'],
                                         rd['fstapi_reader_lz4']['load_1_s']), rd['vs_fstapi_zlib']['load_1']['vtr'], ms, 'faster'))),
        ('Transactions vs FTR',
         f'SystemC TLM · {tlm["writers"]["vtr-rust"]["transactions"] / 1e6:.0f} M transactions · '
         f'{tlm["writers"]["vtr-rust"]["attributes"] / 1e6:.0f} M attributes',
         (('File size', 'FTR', min(w['bytes'] for w in ftr), tlm['writers']['vtr-rust']['bytes'], mib, 'smaller'),
          ('Write time', 'FTR', min(w['wall_s'] for w in ftr), tlm['writers']['vtr-rust']['wall_s'], sec, 'faster'))))


def one_file():
    """Slide 3: FST's and FTR's ideas packed into one VTR file, and what that measures."""
    groups = benchmarks()
    said = '; '.join(f'{label.lower()}: {name} {fmt(them)}, VTR {fmt(v)}, {them / v:.1f} times {verb}'
                     for _, _, rows in groups for label, name, them, v, fmt, verb in rows)
    s = Svg('f', 1136, 446,
            'VTR takes the ideas of two formats: from FST, per-signal value chains, time blocks and frames, hierarchy and '
            'aliases; from FTR, streams and generators, transactions with begin and end times and attributes, and relations. '
            'It packs them into one file of time-ordered signal, transaction and log blocks with a directory at the end, '
            'compressed with zstd in columnar runs, read by random access and recoverable after a crash. '
            f'Benchmarks against the best FST or FTR variant per metric, {groups[0][1]}, and {groups[1][1]}: {said}.')

    for x, name, note, ideas in ((0, 'FST', 'waveforms, GTKWave',
                                  ('value chains per signal', 'time blocks and frames', 'hierarchy, aliases')),
                                 (264, 'FTR', 'transactions, SystemC',
                                  ('streams, generators', 'begin, end, attributes', 'relations between tx'))):
        s.rect(x, 28, 236, 150, 'f-panel', r=12)
        s.text(x + 20, 60, name, 17, f'c-{name.lower()}', weight=600)
        s.text(x + 20, 82, note, 12, 'c-mut')
        if name == 'FST':
            s.clock(x + 152, 50, 8, 8, 8, 'ln-fst')
            s.wave(x + 152, 66, 8, '00111001', 8, 'ln-fst')
        else:
            for n, (a, w) in enumerate(((0, 64), (10, 38), (22, 30))):
                s.rect(x + 152 + a, 44 + 9 * n, w, 6, 'f-ftr', r=2, box=False)
        for n, idea in enumerate(ideas):
            s.path(f'M{x + 20} {110 + 22 * n} H{x + 26}', 'ln-acc')
            s.text(x + 34, 114 + 22 * n, idea, 11, mono=True)

    # Both formats feed one file.
    s.path('M118 178 V202 H382 V178', 'ln-acc')
    s.arrow(250, 202, 250, 228, 'acc')
    s.text(262, 220, 'packed into one file', 10, 'c-mut', mono=True)

    s.rect(0, 234, 500, 212, 'f-lift', r=12)
    s.text(20, 266, 'VTR', 17, 'c-vtr', weight=600)
    s.text(20, 288, 'one time base for waves, transactions and logs', 12, 'c-mut')
    x = 20
    for name, w, cls, ink in (('META', 44, 'f-chip', 'c-ink'), ('STR', 40, 'f-chip', 'c-ink'), ('HIER', 44, 'f-chip', 'c-ink'),
                              ('SIGNAL', 64, 'f-acc', 'c-on-acc'), ('TX', 40, 'f-acc2', 'c-ink'), ('SIGNAL', 64, 'f-acc', 'c-on-acc'),
                              ('TX', 40, 'f-acc2', 'c-ink'), ('LOG', 40, 'f-chip', 'c-ink'), ('DIR', 40, 'f-chip', 'c-acc-ink')):
        s.rect(x, 304, w, 26, cls, r=4)
        s.text(x + w / 2, 321, name, 10, ink, mono=True, anchor='middle')
        x += w + 4
    s.text(20, 350, 'blocks in time order · directory last · no seek-back', 10, 'c-mut', mono=True)
    s.text(20, 380, 'NEW IN VTR', 10, 'c-acc-ink', mono=True, track=1.2)
    for n, item in enumerate(('zstd columnar runs', 'random-access reads', 'background encoder',
                              'crash-recoverable', 'Rust and C APIs', 'stages, logs, clocks')):
        cx, cy = 20 + 156 * (n % 3), 404 + 24 * (n // 3)
        s.path(f'M{cx} {cy - 4} H{cx + 6}', 'ln-acc')
        s.text(cx + 14, cy, item, 11, mono=True)

    # Measured: one bar pair per metric, the competitor's bar at full length, VTR's to the same scale.
    PX, PW, BX, BW = 580, 532, 712, 300
    SAYS = {'File size': 'smaller file', 'Write time': 'faster write', 'Read one signal': 'faster read'}
    s.rect(556, 28, 580, 418, 'f-panel', r=12)
    y = 60
    for g, (title, _, rows) in enumerate(groups):
        if g:
            s.path(f'M{PX} {y - 26} H{PX + PW}', 'ln-hair')
        s.text(PX, y, title, 15, weight=600)
        y += 16
        for label, name, them, v, fmt, _ in rows:
            fmt_name = name.lower()
            s.text(PX, y + 28, f'{them / v:.1f}×', 30, 'c-vtr', weight=600)
            s.text(PX, y + 52, SAYS[label], 12, 'c-mut')
            s.rect(BX, y + 8, BW, 12, f'f-them f-{fmt_name}', r=3, box=False)
            s.text(BX + BW + 8, y + 18, f'{name} {fmt(them)}', 11, f'c-{fmt_name}', mono=True)
            w = round(BW * v / them, 1)
            # What VTR saves stays visible as a hatched ghost out to the competitor's length.
            s.add(f'<rect x="{BX + w - 3}" y="{y + 26.5}" width="{BW - w + 2.5}" height="11" rx="3" class="f-save"'
                  f' fill="url(#f-hatch)"/>')
            s.rect(BX, y + 26, w, 12, 'f-vtr', r=3, box=False)
            s.text(BX + BW + 8, y + 36, f'VTR {fmt(v)}', 11, 'c-vtr', mono=True)
            y += 64
        y += 30
    return s.render('<pattern id="f-hatch" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">'
                    '<rect width="2" height="6" class="f-vtr hatch"/></pattern>')


# Slide 4: what Volna takes from each viewer, and the panel each idea became.
VIEWERS = (('GTKWave', 'fst', 'the FST classic', ('signal tree + list', 'baseline marker'), 'Waves'),
           ('Surfer', 'surfer', 'Rust waveforms', ('tiled workspace', 'event arrows, 0–9 keys'), 'Workspace'),
           ('SCViewer', 'ftr', 'SystemC transactions', ('transaction streams', 'relations as arrows'), 'Transactions'),
           ('Perfetto', 'perfetto', 'system traces', ('summary tracks', 'details follow selection'), 'Summaries'),
           ('Konata', 'konata', 'CPU pipelines', ('stages per instruction', 'stage colour ladder'), 'Pipeline'))
TARGETS = (('Rust + GPUI', ('native desktop app', 'on Zed’s GPU UI toolkit')),
           ('VS Code plugin', ('the WASM build in a webview', 'remote traces via volna-server')),
           ('WASM in CI', ('any browser, nothing to install', 'one link from a failing run')))


def everywhere():
    """Slide 4: five viewers' best ideas feed one Volna core, which runs natively, in VS Code and in CI."""
    said = '; '.join(f'{name} ({note}): {" and ".join(ideas)}, which became Volna\'s {panel.lower()}'
                     for name, _, note, ideas, panel in VIEWERS)
    runs = '; '.join(f'{t}: {", ".join(n)}' for t, n in TARGETS)
    s = Svg('w', 1136, 446,
            f'Five viewers feed one framework. {said}. Volna is one Rust core with every panel, tested headless, '
            f'and it runs in three places: {runs}.')
    CW, GAP = 212, 19
    s.rect(0, 156, 1136, 112, 'f-lift', r=12)
    s.text(36, 246, 'Volna', 22, 'c-vtr', weight=600)
    s.text(112, 246, 'one Rust core for every panel · tested headless', 13, 'c-mut')
    for i, (name, tone, note, ideas, panel) in enumerate(VIEWERS):
        x = i * (CW + GAP)
        cx, gx, gy = x + CW / 2, x + 144, 16
        s.rect(x, 0, CW, 124, 'f-panel', r=12)
        s.text(x + 18, 30, name, 17, f'c-{tone}', weight=600)
        s.text(x + 18, 50, note, 11, 'c-mut')
        if tone == 'fst':
            s.clock(gx, gy + 6, 6.5, 8, 8, 'ln-fst')
            s.wave(gx, gy + 22, 6.5, '00111001', 8, 'ln-fst')
        elif tone == 'surfer':
            for tx, ty, tw, th in ((0, 0, 22, 28), (26, 0, 26, 12), (26, 16, 26, 12)):
                s.rect(gx + tx, gy + ty, tw, th, 'o-surfer', r=2, box=False)
        elif tone == 'ftr':
            for n, (a, w) in enumerate(((0, 36), (10, 24), (20, 32))):
                s.rect(gx + a, gy + 2 + 9 * n, w, 6, 'f-ftr', r=2, box=False)
        elif tone == 'perfetto':
            for a, d, w in ((0, 0, 52), (0, 1, 22), (26, 1, 20), (4, 2, 10), (30, 2, 8)):
                s.rect(gx + a, gy + 2 + 9 * d, w, 6, 'f-perfetto', r=1, box=False)
        else:
            for r in range(3):
                for c in range(3):
                    s.rect(gx + 10 * (r + c), gy + 2 + 9 * r, 8, 6, 'f-konata', r=1, box=False)
        for n, idea in enumerate(ideas):
            s.path(f'M{x + 18} {80 + 22 * n} H{x + 24}', f'ln-{tone}')
            s.text(x + 32, 84 + 22 * n, idea, 11, mono=True)
        # Each viewer's idea drops straight into the panel it became.
        s.path(f'M{cx} 124 V172', f'ln-{tone} feed')
        s.rect(cx - 70, 172, 140, 30, f'f-idea f-{tone}', r=15)
        s.text(cx, 192, panel, 13, 'c-on-acc', weight=600, anchor='middle')

    TW = 368
    for i, (title, notes) in enumerate(TARGETS):
        x = i * (TW + 16)
        s.arrow(x + TW / 2, 268, x + TW / 2, 298, 'acc')
        s.rect(x, 304, TW, 142, 'f-panel', r=12)
        gx, gy = x + 20, 332
        if i < 2:
            s.rect(gx, gy, 124, 86, 'f-chip', r=6, box=False)
        if i == 0:
            for n in range(3):
                s.add(f'<circle cx="{gx + 10 + 9 * n}" cy="{gy + 9}" r="2.5" class="f-hair"/>')
            s.clock(gx + 12, gy + 34, 12.5, 8, 10, 'ln-vtr')
            s.wave(gx + 12, gy + 54, 12.5, '00111100', 10)
            s.wave(gx + 12, gy + 72, 12.5, '00001110', 10)
        elif i == 1:
            s.path(f'M{gx + 18} {gy} V{gy + 86}', 'ln-hair')
            for n in range(3):
                s.rect(gx + 5, gy + 10 + 16 * n, 8, 8, 'f-hair', r=2, box=False)
            s.rect(gx + 18, gy, 56, 16, 'f-lift', r=0, box=False)
            s.clock(gx + 28, gy + 38, 11, 8, 10, 'ln-vtr')
            s.wave(gx + 28, gy + 58, 11, '00111100', 10)
            s.wave(gx + 28, gy + 76, 11, '00001110', 10)
        else:
            for n, (step, ok) in enumerate((('lint', True), ('build', True), ('sim', False))):
                s.rect(gx, gy + 8 + 26 * n, 54, 18, 'f-chip', r=9, box=False)
                s.text(gx + 7, gy + 21 + 26 * n, step, 9, 'c-mut', mono=True)
                s.mark(gx + 39, gy + 22 + 26 * n, '✓' if ok else '×', 12, 'c-ok' if ok else 'c-warn')
            s.arrow(gx + 56, gy + 69, gx + 62, gy + 69, 'warn')
            s.rect(gx + 66, gy + 8, 58, 70, 'f-chip', r=4, box=False)
            s.rect(gx + 71, gy + 13, 48, 7, 'f-hair', r=3, box=False)
            s.wave(gx + 72, gy + 40, 5.75, '00111000', 8, 'ln-vtr')
            s.wave(gx + 72, gy + 58, 5.75, '01100110', 8)
            s.path(f'M{gx + 101} {gy + 28} V{gy + 72}', 'ln-warn')
        s.text(x + 164, 352, title, 17, 'c-vtr', weight=600)
        for n, note in enumerate(notes):
            s.text(x + 164, 378 + 20 * n, note, 12, 'c-mut')
    return s.render()


CAPTIONS = (('L0 · Input', 'cap', 'Your RTL source and its simulation.'), ('L1 · AI skill', 's', SENTENCES[0]),
            ('L2 · VTR + VDB', 's', SENTENCES[1]), ('L3 · Volna', 's', SENTENCES[2]))


def font_face(name, file, weight):
    f = TTFont(FONTS / file)
    opts = subset.Options()
    opts.flavor, opts.layout_features = 'woff2', ['kern']
    opts.name_IDs, opts.notdef_outline = [], True
    text = ''.join(chr(c) for c in range(0x20, 0x7f)) + '·—–’“”…→↑↓←✓×↻≈'
    sub = subset.Subsetter(opts)
    sub.populate(text=text)
    sub.subset(f)
    buf = io.BytesIO()
    f.flavor = 'woff2'
    f.save(buf)
    data = base64.b64encode(buf.getvalue()).decode()
    return (f"@font-face{{font-family:'{name}';font-weight:{weight};font-style:normal;font-display:block;"
            f"src:url(data:font/woff2;base64,{data}) format('woff2')}}")


def main():
    here = Path(__file__).parent
    css, js = (here / 'vtr-pitch.css').read_text(), (here / 'vtr-pitch.js').read_text()
    faces = ''.join((font_face('Plex', 'IBMPlexSans-Regular.ttf', 400),
                     font_face('Plex', 'IBMPlexSans-SemiBold.ttf', 600),
                     font_face('Lilex', 'Lilex-Regular.ttf', 400)))
    copy = ''.join(f'<li class="{k}"><span class="tag">{esc(tag)}</span><p class="{k}">{esc(t)}</p></li>'
                   for tag, k, t in CAPTIONS)
    page = f'''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Level up your traces</title>
<meta name="description" content="Pitch slides for the AI skill, VTR/VDB and Volna: the staircase from your RTL to design insight, the design optimized down to RTL with its trace lifted back up by agents, VTR, one file for FST's waveforms and FTR's transactions, with benchmarks, and Volna, the best of five viewers in one Rust core that runs natively, in VS Code and in CI.">
<!-- Generated by docs/slides/build-vtr-pitch.py; edit the generator, not this file. -->
<style>{faces}
{css}</style>
</head>
<body>
<main class="stage" aria-label="Level up your traces">
<div class="frame"><section class="slide" id="level-up" aria-roledescription="slide" aria-label="1 · Level up your traces">
<h1>{TITLE}</h1><p class="sub">{SUBTITLE}</p>
<figure class="fig">{staircase()}</figure><ol class="copy">{copy}</ol></section>
<section class="slide" id="down-up" aria-roledescription="slide" aria-label="2 · Optimized down, lifted back up" hidden>
<h1>{V_TITLE}</h1><p class="sub">{V_SUBTITLE}</p>
<figure class="fig">{down_up()}</figure></section>
<section class="slide" id="one-file" aria-roledescription="slide" aria-label="3 · Two formats, one modern file" hidden>
<h1>{F_TITLE}</h1><p class="sub">{F_SUBTITLE}</p>
<figure class="fig">{one_file()}</figure></section>
<section class="slide" id="everywhere" aria-roledescription="slide" aria-label="4 · Five viewers, one modern framework" hidden>
<h1>{W_TITLE}</h1><p class="sub">{W_SUBTITLE}</p>
<figure class="fig">{everywhere()}</figure></section></div>
</main>
<nav class="ctl" aria-label="Slides"><button type="button" data-go="0"><b>1</b> Level up</button>
<button type="button" data-go="1"><b>2</b> Down and up</button>
<button type="button" data-go="2"><b>3</b> One file</button>
<button type="button" data-go="3"><b>4</b> Volna</button>
<span class="keys">← → to switch · P to present</span>
<button type="button" class="present" data-present>Present</button></nav>
<p class="sr" aria-live="polite" id="said"></p>
<script>{js}</script>
</body>
</html>
'''
    OUT.write_text(page)
    print(f'{OUT.relative_to(ROOT)}: {len(page) / 1024:.0f} KiB')


if __name__ == '__main__':
    main()
