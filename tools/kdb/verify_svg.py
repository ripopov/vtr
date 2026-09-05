#!/usr/bin/env python3
"""Parse and rasterize all netlist SVG goldens for visual review (requires rsvg-convert)."""
import argparse
from pathlib import Path
import subprocess
import xml.etree.ElementTree as ET


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, default=Path('crates/vtr-kdb/tests/goldens'))
    parser.add_argument('--output', type=Path, default=Path('target/netlist-svg/review'))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    files = sorted(args.input.glob('*.svg'))
    if not files:
        raise SystemExit('no SVG files found')
    ns = '{http://www.w3.org/2000/svg}'
    for svg in files:
        root = ET.parse(svg).getroot()
        assert root.tag == ns+'svg', svg
        assert root.find(ns+'title') is not None, svg
        assert float(root.attrib['width']) > 0 and float(root.attrib['height']) > 0
        nodes, pins = set(), set()
        for element in root.iter():
            assert element.tag != ns+'script', svg
            for key, ids in [('data-node', nodes), ('data-pin', pins)]:
                if key in element.attrib:
                    assert element.attrib[key] not in ids, (svg, key)
                    ids.add(element.attrib[key])
            if element.tag == ns+'polyline':
                points = [tuple(map(float, p.split(','))) for p in element.attrib['points'].split()]
                assert len(points) >= 2, svg
                for (x1, y1), (x2, y2) in zip(points, points[1:]):
                    assert abs(x1-x2) <= .11 or abs(y1-y2) <= .11, (svg, 'diagonal route')
        output = args.output / (svg.stem+'.png')
        subprocess.run(['rsvg-convert', '--width', '1800', '--keep-aspect-ratio', str(svg), '-o', str(output)], check=True)
        assert output.read_bytes().startswith(b'\x89PNG\r\n\x1a\n')
    # Browser-readable review sheet keeps SVG text sharp at arbitrary zoom.
    import html
    entries = '\n'.join(f'<h2>{html.escape(p.stem)}</h2><img src="{html.escape(p.stem)}.png" style="max-width:100%">' for p in files)
    (args.output/'index.html').write_text('<!doctype html><meta charset="utf-8"><title>Netlist SVG review</title>'+entries)
    print(f'Validated and rasterized {len(files)} SVGs to {args.output}')


if __name__ == '__main__':
    main()
