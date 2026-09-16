#!/usr/bin/env python3
"""Alternate fresh-webview RSA loads in baseline/current VS Code test hosts.

Requires Node >=22 and Pillow. Both hosts must use the same 1200x800, DPR 1
geometry and graphics flags; close other editors and disable workspace autosave.
The measurement excludes opening/metadata and covers adding 27 RSA signals plus
three seconds. Reopening creates a fresh WASM heap, not a cold OS file cache.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
from PIL import Image, ImageChops


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('baseline_port', type=int)
    parser.add_argument('current_port', type=int)
    parser.add_argument('output', type=Path)
    parser.add_argument('--runs', type=int, default=3)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error('runs must be positive')
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    fixture = Path(__file__).resolve().parents[3] / 'bench/results/latest/rsa256.vtr'
    fixture.resolve(strict=True)
    driver = Path(__file__).with_name('cdp.mjs')

    def run(port, name, steps):
        path = out / f'{name}.json'
        path.write_text(json.dumps(steps, indent=2)+'\n')
        with (out / f'{name}.log').open('w') as log:
            subprocess.run(['node', str(driver), str(port), str(path)],
                           stdout=log, stderr=subprocess.STDOUT, check=True, timeout=60)
        return (out / f'{name}.log').read_text()

    results = []
    for sample in range(args.runs):
        variants = [('baseline', args.baseline_port), ('current', args.current_port)]
        if sample % 2:
            variants.reverse()
        for variant, port in variants:
            name = f'{variant}-{sample}'
            run(port, name+'-open', [
                {'foreground': True},
                {'eval': "document.querySelector('.tab.active .action-label.codicon-close-small')?.click()"},
                {'key': 'Escape', 'code': 'Escape', 'vk': 27},
                {'key': 'p', 'ctrl': True, 'code': 'KeyP'}, {'wait': 1500},
                {'key': 'a', 'ctrl': True, 'code': 'KeyA'}, {'text': str(fixture)},
                {'wait': 1500}, {'key': 'Enter', 'code': 'Enter', 'vk': 13}, {'wait': 6000},
                {'eval': "[...document.querySelectorAll('a,button')].find(e=>e.textContent.trim()==='Open Anyway')?.click()"},
                {'wait': 300},
                {'eval': "[...document.querySelectorAll('.monaco-list-row')].find(e=>e.textContent.includes('Volna Waveform ViewerDefault'))?.click()"},
                {'wait': 5000},
                {'eval': "[...document.querySelectorAll('.notifications-toasts .action-label')].filter(e=>e.className.includes('notifications-clear')).forEach(e=>e.click())"},
                {'assert': "document.title.includes('rsa256.vtr') && innerWidth===1200 && innerHeight===800 && devicePixelRatio===1 && [...document.querySelectorAll('iframe')].some(e=>{const r=e.getBoundingClientRect();return r.x===48&&r.y===92&&r.width===1152&&r.height===686;})"},
            ])
            log = run(port, name+'-load', [
                {'target': 'vscode-webview://'}, {'context': '!!document.querySelector("canvas")'},
                {'click': [39, 118]}, {'click': [89, 238]}, {'wait': 300},
                {'eval': r'''(async()=>{
                  const m=await import([...document.scripts].map(s=>s.textContent).join('')
                    .match(/from "([^"]+volna\.js)"/)[1]);const wasm=await m.default();
                  const p=window.coldLoad={module:m,wasm,frames:[],tasks:[],
                    start:performance.now(),memoryStart:wasm.memory.buffer.byteLength};
                  let last=performance.now();function tick(){const now=performance.now();
                    p.frames.push(now-last);last=now;p.raf=requestAnimationFrame(tick)}
                  p.raf=requestAnimationFrame(tick);
                  p.observer=new PerformanceObserver(list=>p.tasks.push(...list.getEntries()
                    .map(e=>({start:e.startTime-p.start,duration:e.duration}))));
                  p.observer.observe({type:'longtask'});return true;
                })()'''},
                {'click': [258, 295]}, {'wait': 3000},
                {'eval': '''(()=>{const p=coldLoad;cancelAnimationFrame(p.raf);p.observer.disconnect();
                  p.module.debug_state();const sorted=[...p.frames].sort((a,b)=>a-b);
                  const percentile=q=>sorted[Math.min(sorted.length-1,Math.ceil(q*sorted.length)-1)];
                  return {frames:p.frames.length,observationMs:performance.now()-p.start,
                    medianFrameMs:percentile(.5),p95FrameMs:percentile(.95),p99FrameMs:percentile(.99),
                    maxFrameMs:Math.max(...p.frames),framesOver33ms:p.frames.filter(x=>x>33).length,
                    framesOver50ms:p.frames.filter(x=>x>50).length,longTasks:p.tasks,
                    memoryStart:p.memoryStart,memoryEnd:p.wasm.memory.buffer.byteLength};})()'''},
                {'wait': 250},
            ])
            measurements = [json.loads(line[6:]) for line in log.splitlines() if line.startswith('eval: ')]
            result = next(m for m in measurements if isinstance(m, dict) and 'frames' in m)
            states = re.findall(r'STATE ([^\n]+)', log)
            if not states or 'items=27 loaded=27' not in states[-1]:
                raise SystemExit(f'{name}: not all 27 rows loaded; inspect console')
            result.update(variant=variant, sample=sample, state=states[-1])
            results.append(result)
            (out / 'samples.json').write_text(json.dumps(results, indent=2)+'\n')
            run(port, name+'-shot', [{'target': 'rsa256.vtr'}, {'move': [20, 20]},
                                    {'wait': 300}, {'shot': str(out / f'{name}.png')}])
            print(json.dumps(result), flush=True)
    comparisons = []
    for sample in range(args.runs):
        with Image.open(out / f'baseline-{sample}.png') as a, Image.open(out / f'current-{sample}.png') as b:
            diff = ImageChops.difference(a.convert('RGB').crop((48, 92, 1200, 754)),
                                         b.convert('RGB').crop((48, 92, 1200, 754)))
        pixels = sum(n for n, color in diff.getcolors(diff.width*diff.height) if color != (0, 0, 0))
        comparisons.append({'sample': sample, 'differentPixels': pixels})
        if pixels:
            diff.save(out / f'diff-{sample}.png')
    (out / 'visual.json').write_text(json.dumps(comparisons, indent=2)+'\n')
    if any(c['differentPixels'] for c in comparisons):
        raise SystemExit('Waveform pixels differ; inspect the captured images')


if __name__ == '__main__':
    main()
