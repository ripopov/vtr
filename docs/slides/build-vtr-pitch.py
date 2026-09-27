#!/usr/bin/env python3
"""Generate docs/vtr-pitch.html: the hero slide for the AI skill, VTR/VDB and Volna.

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
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parents[2]
FONTS = ROOT / 'volna/volna-core/assets/fonts'
OUT = ROOT / 'docs/vtr-pitch.html'

TITLE = 'Level up your traces'
SUBTITLE = 'See your hardware run at the level you designed it.'
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
<meta name="description" content="The hero slide for the AI skill, VTR/VDB and Volna: from your RTL to pipeline, sequence and bandwidth views.">
<!-- Generated by docs/slides/build-vtr-pitch.py; edit the generator, not this file. -->
<style>{faces}
{css}</style>
</head>
<body>
<main class="stage" aria-label="Level up your traces">
<div class="frame"><section class="slide" id="pitch" aria-label="Level up your traces">
<h1>{TITLE}</h1><p class="sub">{SUBTITLE}</p>
<figure class="fig">{staircase()}</figure><ol class="copy">{copy}</ol></section></div>
</main>
<nav class="ctl" aria-label="Presentation"><span class="keys">P to present · Esc to leave</span>
<button type="button" class="present" data-present>Present</button></nav>
<script>{js}</script>
</body>
</html>
'''
    OUT.write_text(page)
    print(f'{OUT.relative_to(ROOT)}: {len(page) / 1024:.0f} KiB')


if __name__ == '__main__':
    main()
