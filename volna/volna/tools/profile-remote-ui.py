#!/usr/bin/env python3
"""Profile the 27-signal RSA selection in an isolated VS Code webview.

Open bench/results/latest/rsa256.vtr with Volna first, at 1200x800, DPR 1.
Requires Node >=22. See VERIFICATION.md for graphics flags and limitations.
Writes the replay script, console log, and checked measurements.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("port", type=int)
    parser.add_argument("output", type=Path)
    parser.add_argument("--send-delay-ms", type=int, default=0)
    args = parser.parse_args()
    if not 0 <= args.send_delay_ms <= 100:
        parser.error("send delay must be between 0 and 100 ms")
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    driver = Path(__file__).with_name("cdp.mjs")
    fixture = Path(__file__).resolve().parents[3] / "bench/results/latest/rsa256.vtr"

    def run(name, steps):
        script = out / f"{name}.json"
        script.write_text(json.dumps(steps, indent=2) + "\n")
        with (out / f"{name}.log").open("w") as log:
            subprocess.run(["node", str(driver), str(args.port), str(script)],
                           stdout=log, stderr=subprocess.STDOUT, check=True, timeout=60)
        return (out / f"{name}.log").read_text()

    run("geometry", [{"target": "rsa256.vtr"}, {"foreground": True}, {"assert": """
      innerWidth===1200 && innerHeight===800 && devicePixelRatio===1 &&
      [...document.querySelectorAll('iframe')].some(e=>{
        const r=e.getBoundingClientRect();
        return r.x===48 && r.y===92 && r.width===1152 && r.height===686;
      })"""}])
    setup = r"""(async()=>{
      const module=await import([...document.scripts].map(s=>s.textContent).join('')
        .match(/from "([^"]+volna\.js)"/)[1]);
      const wasm=await module.default();
      window.remoteProfile={module,wasm};
      module.open_remote('rsa256.vtr',JSON.stringify(METADATA));
    })()""".replace("METADATA", json.dumps({"traceUri": fixture.as_uri(), "candidates": None,
                                         "settings": {"autosave": "off", "linkByDefault": True}}))
    start = """(()=>{
      const p=window.remoteProfile;
      p.sent=0;p.received=0;p.sendBytes=0;p.receiveBytes=0;
      p.firstSend=null;p.lastSend=null;p.frames=[];p.tasks=[];
      p.memoryStart=p.wasm.memory.buffer.byteLength;
      p.originalSend=window.volnaTraceSend;
      const transmit=args=>{
        const now=performance.now();p.sent++;p.sendBytes+=args[1].byteLength;
        p.firstSend??=now;p.lastSend=now;p.originalSend(...args);
      };
      window.volnaTraceSend=(...args)=>DELAY ? setTimeout(()=>transmit(args),DELAY) : transmit(args);
      p.message=e=>{if(e.data?.type==='traceFrame'){
        p.received++;p.receiveBytes+=e.data.bytes.byteLength;
      }};
      addEventListener('message',p.message);
      p.observer=new PerformanceObserver(list=>p.tasks.push(...list.getEntries()
        .map(e=>({start:e.startTime,duration:e.duration}))));
      p.observer.observe({type:'longtask'});
      let last=performance.now();
      function tick(){const now=performance.now();p.frames.push({at:now,gap:now-last});
        last=now;p.raf=requestAnimationFrame(tick);}
      p.raf=requestAnimationFrame(tick);
      return {visible:document.visibilityState,memoryStart:p.memoryStart};
    })()""".replace("DELAY", str(args.send_delay_ms))
    steps = [{"target": "vscode-webview://"}, {"context": '!!document.querySelector("canvas")'},
             {"eval": setup}, {"wait": 1500}, {"eval": start},
             {"click": [39, 118]}, {"click": [89, 238]}, {"click": [258, 295]},
             {"wait": 1500}, {"click": [912, 164]}, {"wheel": [882, 158, 0, -140, "ctrl"]},
             {"eval": "window.remoteProfile.module.debug_state()"}, {"wait": 250},
             {"eval": """(async()=>{
               const p=window.remoteProfile,deadline=performance.now()+40000;
               while(p.received<250 || p.sent<=p.received || performance.now()-p.lastSend<300){
                 if(performance.now()>deadline)throw Error('RSA transfer did not complete');
                 await new Promise(r=>setTimeout(r,100));
               }
               p.module.debug_state();
               const frames=p.frames.filter(f=>f.at>=p.firstSend && f.at<=p.lastSend+100);
               p.result={sent:p.sent,received:p.received,sendBytes:p.sendBytes,
                 receiveBytes:p.receiveBytes,loadMs:p.lastSend-p.firstSend,
                 memoryStart:p.memoryStart,memoryEnd:p.wasm.memory.buffer.byteLength,
                 frames:frames.length,maxFrameMs:Math.max(...frames.map(f=>f.gap)),
                 longTasks:p.tasks.filter(t=>t.start>=p.firstSend && t.start<=p.lastSend+100)};
               cancelAnimationFrame(p.raf);p.observer.disconnect();
               removeEventListener('message',p.message);
               return p.result;
             })()"""}, {"wait": 250},
             {"click": [800, 164]}, {"wheel": [882, 158, 0, 140, "ctrl"]}, {"wait": 300},
             {"eval": """(()=>{const p=window.remoteProfile;
               window.volnaTraceSend=p.originalSend;
               if(p.sent!==p.result.sent)throw Error('Loaded navigation sent data requests');
               return {loadedNavigationSends:p.sent-p.result.sent};})()"""}]
    log = run("profile", steps)
    measurements = [json.loads(line[6:]) for line in log.splitlines() if line.startswith("eval: ")]
    result = next(m for m in measurements if "loadMs" in m)
    states = re.findall(r"STATE ([^\n]+)", log)[-2:]
    if len(states) != 2 or "items=27 loaded=27" not in states[-1]:
        raise SystemExit("Expected all 27 RSA rows ready; inspect profile.log")
    if args.send_delay_ms:
        loaded = re.search(r"loaded=(\d+)", states[0])
        if not loaded or not 0 < int(loaded[1]) < 27 or "cursor=Some(" not in states[0]:
            raise SystemExit("Did not establish cursor response during partial loading")
        if "viewport=(109035,287224)" not in states[0]:
            raise SystemExit("Zoom was not applied during partial loading")
    result.update(fixture="bench/results/latest/rsa256.vtr", scope="TOP.Testbench.i_rsa.i_RSAMont",
                  sendDelayMs=args.send_delay_ms, states=states, loadedNavigationSends=0,
                  measurement="Data request through last acknowledgement; metadata/startup excluded")
    (out / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    if result["maxFrameMs"] >= 50 or result["longTasks"]:
        raise SystemExit("Frame/task budget exceeded; inspect graphics and load traces")


if __name__ == "__main__":
    main()
