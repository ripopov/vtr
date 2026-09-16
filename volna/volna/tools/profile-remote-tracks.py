#!/usr/bin/env python3
"""Measure complete stream loads in a VS Code build with remote-profile enabled.

Open the supplied trace in an isolated Volna editor first. This diagnostic
consumer uses the production document, relay, decoder and client indexes.
Requires Node >=22; see VERIFICATION.md for build and graphics setup.
"""
import argparse
import json
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('port', type=int)
    parser.add_argument('trace', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--memory-mib', type=int, default=512)
    parser.add_argument('--object-mib', type=int, default=256)
    args = parser.parse_args()
    trace = args.trace.resolve(strict=True)
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    # open_remote cannot change the path authorized by the extension. Verify
    # the real active editor before labelling measurements with this fixture.
    editor = out / 'editor.json'
    editor.write_text(json.dumps([{'target': trace.name}, {'foreground': True}, {'assert':
        "document.querySelector('.tab.active')?.getAttribute('aria-label') === "
        + json.dumps(trace.name)}], indent=2)+'\n')
    with (out / 'editor.log').open('w') as log:
        subprocess.run(['node', str(Path(__file__).with_name('cdp.mjs')),
                        str(args.port), str(editor)], stdout=log, stderr=subprocess.STDOUT,
                       check=True, timeout=15)
    script = out / 'profile.json'
    expression = r'''(async()=>{
      const module=await import([...document.scripts].map(s=>s.textContent).join('')
        .match(/from "([^"]+volna\.js)"/)[1]);
      if(typeof module.profile_tracks!=='function')throw Error('Build with remote-profile');
      const wasm=await module.default();
      let state,firstReady=null,start;
      const pause=ms=>new Promise(r=>setTimeout(r,ms));
      window.volnaProfileResult=json=>{
        state=JSON.parse(json);
        if(start && state.ready && firstReady===null)firstReady=performance.now()-start;
      };
      module.open_remote(NAME,JSON.stringify(METADATA));
      const deadline=performance.now()+45000;
      while(!state?.streams){
        if(performance.now()>deadline)throw Error('Metadata did not open');
        module.profile_tracks('state');await pause(100);
      }
      const originalSend=window.volnaTraceSend;
      let sent=0,received=0,sendBytes=0,receiveBytes=0,lastSend=0;
      let raf,last=performance.now();const frames=[],tasks=[];
      const observer=new PerformanceObserver(list=>tasks.push(...list.getEntries()
        .map(e=>({start:e.startTime,duration:e.duration}))));
      observer.observe({type:'longtask'});
      function tick(){const now=performance.now();frames.push({at:now,gap:now-last});
        last=now;raf=requestAnimationFrame(tick);}
      const message=e=>{if(e.data?.type==='traceFrame'){
        received++;receiveBytes+=e.data.bytes.byteLength;
      }};
      addEventListener('message',message);
      window.volnaTraceSend=(...args)=>{
        sent++;sendBytes+=args[1].byteLength;lastSend=performance.now();
        return originalSend(...args);
      };
      const memoryStart=wasm.memory.buffer.byteLength;
      start=performance.now();raf=requestAnimationFrame(tick);
      try {
        module.profile_tracks('load');await pause(100);
        let timedOut=false;
        while(state.loading || state.ready+state.errors.length!==state.streams){
          if(performance.now()>deadline){timedOut=true;break;}
          module.profile_tracks('state');await pause(100);
        }
        await pause(100);
        const loadEnd=performance.now(),loaded=state;
        const result={loaded,timedOut,firstReadyMs:firstReady,totalMs:lastSend-start,
          sent,received,sendBytes,receiveBytes,memoryStart,
          memoryLoaded:wasm.memory.buffer.byteLength};
        if(timedOut)return {...result,
          observationMs:loadEnd-start,frames:frames.length,
          maxFrameMs:Math.max(...frames.map(f=>f.gap)),longTasks:tasks};
        const releaseStart=performance.now();
        module.profile_tracks('release');
        while(state.action!=='release')await pause(10);
        result.releaseMs=performance.now()-releaseStart;
        result.released=state;
        await pause(100);
        result.maxFrameMs=Math.max(...frames.filter(f=>f.at<=loadEnd).map(f=>f.gap));
        result.frames=frames.filter(f=>f.at<=loadEnd).length;
        result.longTasks=tasks.filter(t=>t.start<=loadEnd);
        result.releaseLongTasks=tasks.filter(t=>t.start>=releaseStart);
        result.releaseSends=sent-result.sent;
        result.memoryReleased=wasm.memory.buffer.byteLength;
        return result;
      } finally {
        cancelAnimationFrame(raf);observer.disconnect();
        removeEventListener('message',message);window.volnaTraceSend=originalSend;
        delete window.volnaProfileResult;
      }
    })()'''.replace('NAME', json.dumps(trace.name)).replace('METADATA', json.dumps({
        'traceUri': trace.as_uri(), 'candidates': None,
        'settings': {'autosave': 'off', 'linkByDefault': True,
                     'remote': {'memoryMiB': args.memory_mib, 'objectMiB': args.object_mib}}}))
    steps = [{'target': 'vscode-webview://'}, {'context': '!!document.querySelector("canvas")'},
             {'eval': expression}]
    script.write_text(json.dumps(steps, indent=2)+'\n')
    with (out / 'profile.log').open('w') as log:
        subprocess.run(['node', str(Path(__file__).with_name('cdp.mjs')),
                        str(args.port), str(script)], stdout=log, stderr=subprocess.STDOUT,
                       check=True, timeout=60)
    result = next(json.loads(line[6:]) for line in (out / 'profile.log').read_text().splitlines()
                  if line.startswith('eval: {"loaded"'))
    result.update(fixture=str(trace), memoryMiB=args.memory_mib, objectMiB=args.object_mib)
    (out / 'results.json').write_text(json.dumps(result, indent=2)+'\n')
    print(json.dumps(result, indent=2))
    if result['timedOut']:
        raise SystemExit('Observation deadline exceeded; active document retained, see measurements')
    if result['released']['ready'] or result['released']['loading'] or result['releaseSends']:
        raise SystemExit('Release did not remove demand locally')
    if result['maxFrameMs']>=50 or result['longTasks'] or result['releaseLongTasks']:
        raise SystemExit('Frame/task budget exceeded; inspect saved measurements')


if __name__ == '__main__':
    main()
