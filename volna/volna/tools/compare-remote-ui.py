#!/usr/bin/env python3
"""Compare two isolated VS Code viewers using the same picorv32.vtr pose.

Requires Node >= 22 and Pillow. See ../VERIFICATION.md for window setup.
Saves the exact CDP scripts, console logs, screenshots and comparison results.
"""

import argparse
import json
from pathlib import Path
import re
import subprocess

from PIL import Image, ImageChops


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline_port", type=int)
    parser.add_argument("current_port", type=int)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    driver = Path(__file__).with_name("cdp.mjs")

    def run(port, name, steps):
        script = output / f"{name}.json"
        script.write_text(json.dumps(steps, indent=2) + "\n")
        with (output / f"{name}.log").open("w") as log:
            subprocess.run(["node", str(driver), str(port), str(script)],
                           stdout=log, stderr=subprocess.STDOUT, check=True,
                           timeout=60)
        return (output / f"{name}.log").read_text()

    top = [{"target": "picorv32.vtr"}, {"foreground": True}]
    content = [{"target": "vscode-webview://"},
               {"context": '!!document.querySelector("canvas")'}]
    poses = {}
    for variant, port in [("baseline", args.baseline_port),
                          ("current", args.current_port)]:
        run(port, variant + "-geometry", top + [{"assert": """
          innerWidth === 1200 && innerHeight === 800 && devicePixelRatio === 1
          && [...document.querySelectorAll('iframe')].some(e => {
            const r=e.getBoundingClientRect();
            return r.x===48 && r.y===92 && r.width===1152 && r.height===686;
          })"""}])
        state = run(port, variant + "-state", content + [{"eval": r"""
          import([...document.scripts].map(s=>s.textContent).join('')
            .match(/from "([^"]+volna\.js)"/)[1]).then(m=>m.debug_state())
          """}, {"wait": 250}])
        states = re.findall(r"STATE ([^\n]+)", state)
        if not states or "items=8 loaded=8" not in states[-1]:
            raise SystemExit(f"{variant}: expected eight loaded root signals; see state log")
        if variant == "current":
            run(port, "count-start", content + [
                {"assert": "typeof window.volnaTraceSend === 'function'"},
                {"eval": """
                  window.volnaComparisonSends=0;
                  window.volnaComparisonOriginalSend=window.volnaTraceSend;
                  window.volnaTraceSend=(...args)=>{
                    window.volnaComparisonSends++;
                    return window.volnaComparisonOriginalSend(...args);
                  }; true"""}])
        initial = output / f"{variant}-initial.png"
        navigated = output / f"{variant}-navigated.png"
        filtered = output / f"{variant}-filtered.png"
        poses[variant] = [initial, navigated, filtered]
        run(port, variant + "-actions", top + [
            {"move": [20, 20]}, {"wait": 300}, {"shot": str(initial)},
            {"click": [900, 256]},
            {"wheel": [930, 250, 0, -260, "ctrl"]},
            {"wheel": [930, 250, 220, 0]},
            {"move": [20, 20]}, {"wait": 300}, {"shot": str(navigated)},
            {"click": [155, 418]},
            # GPUI consumes keyboard events; insertText does not exercise it.
            {"key": "c", "code": "KeyC"},
            {"key": "l", "code": "KeyL"},
            {"key": "k", "code": "KeyK"},
            {"key": "Tab", "code": "Tab", "vk": 9},
            {"move": [20, 20]}, {"wait": 300}, {"shot": str(filtered)},
        ])
        if variant == "current":
            run(port, "count-end", content + [
                {"eval": "window.volnaTraceSend=window.volnaComparisonOriginalSend; window.volnaComparisonSends"},
                {"assert": "window.volnaComparisonSends === 0"},
            ])

    results = {"remote_messages_during_interaction": 0, "poses": []}
    # Exclude host breadcrumbs (different paths) and the live frame-time label.
    bounds = (48, 92, 1200, 754)
    for baseline, current in zip(poses["baseline"], poses["current"]):
        with Image.open(baseline) as a, Image.open(current) as b:
            diff = ImageChops.difference(a.convert("RGB").crop(bounds),
                                         b.convert("RGB").crop(bounds))
        count = sum(count for count, pixel in diff.getcolors(diff.width * diff.height)
                    if pixel != (0, 0, 0))
        results["poses"].append({"pose": baseline.stem.removeprefix("baseline-"),
                                 "different_pixels": count})
        if count:
            diff.save(output / (baseline.stem + "-diff.png"))
    (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps(results, indent=2))
    if any(p["different_pixels"] for p in results["poses"]):
        raise SystemExit("Viewer pixels differ; inspect the captures and diff images.")


if __name__ == "__main__":
    main()
