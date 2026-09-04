#!/usr/bin/env python3
"""VTR benchmark orchestrator.

Builds the Rust crates and the C/C++ harnesses, prepares the workloads, runs every
writer/reader comparison and writes raw JSON results plus a Markdown report.

    python3 bench/run.py all              # everything (needs verilator for the RSA256 workload)
    python3 bench/run.py prepare          # only workload preparation
    python3 bench/run.py run              # only measurements (workloads must exist)
    python3 bench/run.py report           # only re-render docs/BENCHMARK_RESULTS.md

Options: --scale small|full (default full), --out DIR (default bench/results/latest),
         --workloads NAME[,NAME...] to restrict, --repeat N (default 3, best-of).
"""
import argparse
import datetime
import json
import os
import platform
import shutil
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WORK = os.path.join(ROOT, "bench", "workloads", "gen")
BUILD = os.path.join(ROOT, "bench", "build")
TARGET = os.path.join(ROOT, "target", "release")
VTR_BENCH = os.path.join(TARGET, "vtr-bench")
VTR_CLI = os.path.join(TARGET, "vtr")


def sh(cmd, cwd=ROOT, capture=False, env=None, check=True):
    print("+", " ".join(cmd), flush=True)
    r = subprocess.run(cmd, cwd=cwd, capture_output=capture, text=True, env=env)
    if check and r.returncode != 0:
        if capture:
            print(r.stdout)
            print(r.stderr)
        raise SystemExit(f"command failed: {' '.join(cmd)}")
    return r


def json_out(cmd, cwd=ROOT):
    r = sh(cmd, cwd=cwd, capture=True)
    out = r.stdout
    start = out.find("{")
    return json.loads(out[start:])


def build():
    sh(["cargo", "build", "--release"])
    os.makedirs(BUILD, exist_ok=True)
    sh(["cmake", os.path.join(ROOT, "bench", "cpp"), "-DCMAKE_BUILD_TYPE=Release"], cwd=BUILD)
    sh(["make", "-j8"], cwd=BUILD)


# ---------------------------------------------------------------------------
# Workloads
# ---------------------------------------------------------------------------

RTL_WORKLOADS = {
    # name: (kind, args, description)
    "scr1_axi": ("fst", "ext/wavepeek/web/playground/assets/scr1_axi.fst",
                 "SCR1 RISC-V core with AXI testbench, Verilator FST fixture from the wavepeek repository (real RTL, small design)"),
    "rsa256": ("verilator", "200000",
               "RSA-256 Montgomery multiplier from libfstwriter's integration tests driven with continuous pseudo-random operands (bench/workloads/rsa256/RSA_bench_tb.sv), Verilator, 200k cycles (real RTL, wide datapaths)"),
    "rsa256_long": ("verilator", "2000000",
                    "Same RSA-256 design and driver, 2M cycles (real RTL, long simulation)"),
    "scr1_x8": ("replicate", ("scr1_axi", 8),
                "8 perturbed copies of the SCR1 trace under separate top scopes (11.6k signals; models a multi-core SoC)"),
    "long_sparse": ("gen", ("long_sparse", "5000"),
                    "synthetic: 20k signals, 500k time steps, ~0.1% of signals change per step (long run, few active signals)"),
    "many_active": ("gen", ("many_active", "50"),
                    "synthetic: 200k signals, 1000 cycles, 30% of signals change each cycle (short run, many active signals)"),
    "wide_bus": ("gen", ("wide_bus", "20"),
                 "synthetic: 64 buses of 256..2048 bits toggling most cycles (wide vectors)"),
}
SMALL_SET = ["scr1_axi", "rsa256", "long_sparse", "wide_bus"]

TX_WORKLOADS = {
    "tlm_1m": ("tlm", "1000000", "synthetic TLM: 1M CPU instructions -> NoC packets -> slave accesses (3M transactions, ~2.3M relations, 14 attributes per chain)"),
    "ooo_1m": ("kanata", "1000000", "synthetic Kanata pipeline log of a 4-wide out-of-order core, 1M instructions (~8M stages, ~9M relations)"),
    "kanata_sample2": ("kanata-file", "ext/Konata/docs/kanata-sample-2.log.gz", "Konata's bundled sample trace (4k instructions)"),
}
SMALL_TX = ["tlm_1m", "kanata_sample2"]


def prepare(names, scale):
    os.makedirs(WORK, exist_ok=True)
    info = {}
    for name in names:
        if name in RTL_WORKLOADS:
            kind, arg, desc = RTL_WORKLOADS[name]
            rpl = os.path.join(WORK, f"{name}.rpl")
            if kind == "fst":
                src = os.path.join(ROOT, arg)
                if not os.path.exists(rpl):
                    sh([VTR_BENCH, "prepare-fst", src, rpl])
                info[name] = {"source_fst": src}
            elif kind == "verilator":
                if shutil.which("verilator") is None:
                    print(f"skipping {name}: verilator not found")
                    continue
                exe = os.path.join(WORK, "rsa256_build", "verilated", "rsa_tb")
                if not os.path.exists(exe):
                    sh([os.path.join(ROOT, "bench", "workloads", "rsa256", "build.sh"), os.path.join(WORK, "rsa256_build")])
                fst = os.path.join(WORK, f"{name}.fst")
                if not os.path.exists(fst):
                    sh([exe, fst, arg])
                if not os.path.exists(rpl):
                    sh([VTR_BENCH, "prepare-fst", fst, rpl])
                info[name] = {"source_fst": fst, "cycles": int(arg)}
            elif kind == "replicate":
                base, n = arg
                base_src = os.path.join(ROOT, RTL_WORKLOADS[base][1])
                if not os.path.exists(rpl):
                    sh([VTR_BENCH, "prepare-fst", base_src, rpl, "--replicate", str(n)])
                info[name] = {"replicate": n, "base": base}
            elif kind == "gen":
                g, sc = arg
                if not os.path.exists(rpl):
                    sh([VTR_BENCH, "gen", g, sc, rpl])
                info[name] = {"generator": g, "scale": sc}
            st = json_out([VTR_BENCH, "replay-info", rpl])
            info[name].update(st)
            info[name]["description"] = desc
        elif name in TX_WORKLOADS:
            kind, arg, desc = TX_WORKLOADS[name]
            if kind == "tlm":
                p = os.path.join(WORK, f"{name}.txr")
                if not os.path.exists(p):
                    sh([VTR_BENCH, "gen-tlm", p, arg])
                info[name] = {"file": p, "description": desc}
            elif kind == "kanata":
                p = os.path.join(WORK, f"{name}.log")
                if not os.path.exists(p):
                    sh([VTR_BENCH, "gen-kanata", p, arg])
                info[name] = {"file": p, "description": desc}
            else:
                src = os.path.join(ROOT, arg)
                if src.endswith(".gz"):
                    # The C++ harness reads plain text; keep a decompressed copy next to the workloads.
                    import gzip
                    p = os.path.join(WORK, f"{name}.log")
                    if not os.path.exists(p):
                        with gzip.open(src, "rb") as fi, open(p, "wb") as fo:
                            shutil.copyfileobj(fi, fo)
                    info[name] = {"file": p, "source": src, "description": desc}
                else:
                    info[name] = {"file": src, "description": desc}
    return info


# ---------------------------------------------------------------------------
# Measurements
# ---------------------------------------------------------------------------

def best_of(n, fn):
    best = None
    for _ in range(n):
        r = fn()
        if best is None or r["wall_s"] < best["wall_s"]:
            best = r
    return best


def run_rtl(name, info, repeat, out_dir, reads_only=False, previous=None):
    rpl = os.path.join(WORK, f"{name}.rpl")
    res = {"workload": name, "info": info, "writers": {}, "readers": {}}
    files = {}
    fw = os.path.join(BUILD, "fst_write")
    vw = os.path.join(BUILD, "vtr_write")

    def w(label, cmd, path):
        files[label] = path
        if reads_only:
            # Reuse the kept outputs and the previous writer results.
            if previous is not None:
                res["writers"] = previous.get("writers", {})
                if "source_fst_bytes" in previous:
                    res["source_fst_bytes"] = previous["source_fst_bytes"]
            return
        r = best_of(repeat, lambda: json_out(cmd))
        res["writers"][label] = r
        print(f"  {name} {label:14s} {r['wall_s']:.3f}s wall {r.get('cpu_s', 0):.3f}s cpu {r['bytes']:>12} bytes", flush=True)

    w("fstapi-lz4", [fw, rpl, os.path.join(out_dir, f"{name}_fstapi_lz4.fst"), "fstapi", "lz4"], os.path.join(out_dir, f"{name}_fstapi_lz4.fst"))
    w("fstapi-zlib", [fw, rpl, os.path.join(out_dir, f"{name}_fstapi_zlib.fst"), "fstapi", "zlib"], os.path.join(out_dir, f"{name}_fstapi_zlib.fst"))
    w("fstcpp-lz4", [fw, rpl, os.path.join(out_dir, f"{name}_fstcpp.fst"), "fstcpp"], os.path.join(out_dir, f"{name}_fstcpp.fst"))
    w("vtr-c", [vw, rpl, os.path.join(out_dir, f"{name}_c.vtr")], os.path.join(out_dir, f"{name}_c.vtr"))
    w("vtr-c-inline", [vw, rpl, os.path.join(out_dir, f"{name}_c_inline.vtr"), "zstd", "inline"], os.path.join(out_dir, f"{name}_c_inline.vtr"))
    w("vtr-rust", [VTR_BENCH, "write", rpl, os.path.join(out_dir, f"{name}.vtr")], os.path.join(out_dir, f"{name}.vtr"))
    w("vtr-rust-inline", [VTR_BENCH, "write", rpl, os.path.join(out_dir, f"{name}_inline.vtr"), "--no-background"], os.path.join(out_dir, f"{name}_inline.vtr"))
    w("vtr-lz4", [VTR_BENCH, "write", rpl, os.path.join(out_dir, f"{name}_lz4.vtr"), "--codec", "lz4"], os.path.join(out_dir, f"{name}_lz4.vtr"))
    # Reads: wellen (what wavepeek uses) vs VTR, on the GTKWave-default FST (zlib) and, when the
    # workload came from a simulator, on the original simulator-written FST as well.
    vtr_file = files["vtr-rust"]
    res["readers"]["vs_fstapi_zlib"] = json_out([VTR_BENCH, "read", files["fstapi-zlib"], vtr_file])
    if "source_fst" in info:
        res["readers"]["vs_source_fst"] = json_out([VTR_BENCH, "read", info["source_fst"], vtr_file])
    plan = os.path.join(out_dir, f"{name}_plan.txt")
    with open(plan, "w") as f:
        f.write(sh([VTR_BENCH, "plan", vtr_file], capture=True).stdout)
    res["readers"]["fstapi_reader_zlib"] = json_out([os.path.join(BUILD, "fst_read"), files["fstapi-zlib"], plan])
    res["readers"]["fstapi_reader_lz4"] = json_out([os.path.join(BUILD, "fst_read"), files["fstapi-lz4"], plan])
    # Sizes of the original simulator FST for reference.
    if "source_fst" in info:
        res["source_fst_bytes"] = os.path.getsize(info["source_fst"])
    # Remove bulky outputs but keep one VTR and the zlib FST for inspection (and reads-only reruns).
    for label, path in files.items():
        if label not in ("vtr-rust", "fstapi-zlib"):
            try:
                os.remove(path)
            except OSError:
                pass
    return res


def run_tx(name, info, repeat, out_dir):
    res = {"workload": name, "info": info, "writers": {}, "readers": {}}
    kind = TX_WORKLOADS[name][0]
    src = info["file"]
    if kind == "tlm":
        res["writers"]["ftr-lz4"] = best_of(repeat, lambda: json_out([os.path.join(BUILD, "ftr_write"), src, os.path.join(out_dir, f"{name}.ftr")]))
        res["writers"]["ftr-raw"] = best_of(repeat, lambda: json_out([os.path.join(BUILD, "ftr_write"), src, os.path.join(out_dir, f"{name}_raw.ftr"), "raw"]))
        res["writers"]["vtr-rust"] = best_of(repeat, lambda: json_out([VTR_BENCH, "tx-write", src, os.path.join(out_dir, f"{name}.vtr")]))
        res["writers"]["vtr-rust-inline"] = best_of(repeat, lambda: json_out([VTR_BENCH, "tx-write", src, os.path.join(out_dir, f"{name}_inline.vtr"), "--no-background"]))
        os.remove(os.path.join(out_dir, f"{name}_raw.ftr"))
    else:
        res["writers"]["ftr-lz4"] = best_of(repeat, lambda: json_out([os.path.join(BUILD, "konata_ftr"), src, os.path.join(out_dir, f"{name}.ftr")]))
        res["writers"]["vtr-rust"] = best_of(repeat, lambda: json_out([VTR_BENCH, "kanata-write", src, os.path.join(out_dir, f"{name}.vtr")]))
        res["writers"]["vtr-rust-inline"] = best_of(repeat, lambda: json_out([VTR_BENCH, "kanata-write", src, os.path.join(out_dir, f"{name}_inline.vtr"), "--no-background"]))
        res["source_bytes"] = os.path.getsize(src)
        if "source" in info:
            res["source_bytes_gz"] = os.path.getsize(info["source"])
    for label, r in res["writers"].items():
        print(f"  {name} {label:14s} {r['wall_s']:.3f}s wall {r.get('cpu_s', 0):.3f}s cpu {r['bytes']:>12} bytes", flush=True)
    res["readers"]["vtr"] = json_out([VTR_BENCH, "tx-read", os.path.join(out_dir, f"{name}.vtr")])
    return res


def machine_info():
    cpu = ""
    try:
        for line in open("/proc/cpuinfo"):
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    except OSError:
        pass
    mem = ""
    try:
        for line in open("/proc/meminfo"):
            if line.startswith("MemTotal"):
                mem = line.split(":", 1)[1].strip()
                break
    except OSError:
        pass
    ver = lambda cmd: subprocess.run(cmd, capture_output=True, text=True).stdout.strip().splitlines()[0] if shutil.which(cmd[0]) else "n/a"
    return {
        "cpu": cpu, "cores": os.cpu_count(), "memory": mem, "kernel": platform.release(),
        "rustc": ver(["rustc", "--version"]), "gcc": ver(["gcc", "--version"]), "verilator": ver(["verilator", "--version"]),
        "date": datetime.datetime.now().isoformat(timespec="seconds"),
    }


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------

def fmt_bytes(b):
    if b >= 1 << 30:
        return f"{b / (1 << 30):.2f} GiB"
    if b >= 1 << 20:
        return f"{b / (1 << 20):.2f} MiB"
    if b >= 1 << 10:
        return f"{b / (1 << 10):.1f} KiB"
    return f"{b} B"


def render(results, path):
    m = results["machine"]
    L = []
    L.append("# VTR benchmark results\n")
    L.append("Generated by `bench/run.py`; raw data in `bench/results/latest/results.json`. See `docs/BENCHMARKS.md` for the methodology.\n")
    L.append(f"Machine: {m['cpu']} ({m['cores']} threads), {m['memory']}, Linux {m['kernel']}; {m['rustc']}; {m['gcc']}; verilator: {m['verilator']}. Run on {m['date']}.\n")
    L.append("All timings are best-of-N wall-clock seconds of the write loop plus close (writers) or of the query (readers); `cpu` is user+system CPU time of the whole process, including VTR's background encoder thread.\n")
    rtl = [r for r in results["rtl"]]
    if rtl:
        L.append("## Signal workloads: file size\n")
        L.append("| workload | signals | changes | time steps | simulator FST | fstapi LZ4 | fstapi zlib | libfstwriter LZ4 | **VTR** (zstd-3) | VTR lz4 | VTR vs best FST |")
        L.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
        for r in rtl:
            w = r["writers"]
            best_fst = min(w["fstapi-lz4"]["bytes"], w["fstapi-zlib"]["bytes"], w["fstcpp-lz4"]["bytes"])
            src = fmt_bytes(r["source_fst_bytes"]) if "source_fst_bytes" in r else "-"
            L.append(f"| {r['workload']} | {r['info']['signals']} | {r['info']['changes']:,} | {r['info']['time_steps']:,} | {src} | {fmt_bytes(w['fstapi-lz4']['bytes'])} | {fmt_bytes(w['fstapi-zlib']['bytes'])} | {fmt_bytes(w['fstcpp-lz4']['bytes'])} | **{fmt_bytes(w['vtr-rust']['bytes'])}** | {fmt_bytes(w['vtr-lz4']['bytes'])} | {w['vtr-rust']['bytes'] / best_fst * 100:.0f}% |")
        L.append("\n## Signal workloads: write time (seconds, lower is better)\n")
        L.append("| workload | fstapi LZ4 | fstapi zlib | libfstwriter LZ4 | **VTR C API** | VTR C inline | **VTR Rust** | VTR Rust inline | VTR vs fastest FST |")
        L.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
        for r in rtl:
            w = r["writers"]
            best_fst = min(w["fstapi-lz4"]["wall_s"], w["fstapi-zlib"]["wall_s"], w["fstcpp-lz4"]["wall_s"])
            L.append(f"| {r['workload']} | {w['fstapi-lz4']['wall_s']:.3f} | {w['fstapi-zlib']['wall_s']:.3f} | {w['fstcpp-lz4']['wall_s']:.3f} | **{w['vtr-c']['wall_s']:.3f}** ({w['vtr-c']['cpu_s']:.2f} cpu) | {w['vtr-c-inline']['wall_s']:.3f} | **{w['vtr-rust']['wall_s']:.3f}** ({w['vtr-rust']['cpu_s']:.2f} cpu) | {w['vtr-rust-inline']['wall_s']:.3f} | {best_fst / w['vtr-rust']['wall_s']:.2f}x faster |")
        L.append("\n## Signal workloads: read and navigation (milliseconds, lower is better)\n")
        L.append("wellen is the FST reader used by wavepeek; fstapi is GTKWave's C reader. FST input = fstapi zlib file (GTKWave default); VTR input = the VTR file written above. Loads and value queries start from a fresh open each time (one-shot CLI model).\n")
        for r in rtl:
            rd = r["readers"]["vs_fstapi_zlib"]
            fa = r["readers"]["fstapi_reader_zlib"]
            L.append(f"### {r['workload']}\n")
            L.append("| query | wellen (FST) | fstapi (FST) | **VTR** | VTR vs wellen |")
            L.append("|---|---:|---:|---:|---:|")
            rows = [
                ("open", rd["open"]["fst_wellen"], fa["open_s"], rd["open"]["vtr"]),
                ("hierarchy walk (all vars, full paths)", rd["hierarchy"]["fst_wellen"], fa["hierarchy_s"], rd["hierarchy"]["vtr"]),
                ("load 1 signal", rd["load_1"]["fst_wellen"], fa["load_1_s"], rd["load_1"]["vtr"]),
                ("load 10 signals", rd["load_10"]["fst_wellen"], fa["load_10_s"], rd["load_10"]["vtr"]),
                ("load 100 signals", rd["load_100"]["fst_wellen"], fa["load_100_s"], rd["load_100"]["vtr"]),
                ("load 1000 signals", rd["load_1000"]["fst_wellen"], fa["load_1000_s"], rd["load_1000"]["vtr"]),
                ("value at time, 10 signals x 100 times", rd["value_at_10x100"]["fst_wellen"], fa["value_at_s"], rd["value_at_10x100"]["vtr"]),
                ("changes of 1 signal in a 1% window", rd["changes_window_1pct"]["fst_wellen"], None, rd["changes_window_1pct"]["vtr"]),
                ("condition search (posedge clk && bus==v)", rd["condition_search"]["fst_wellen"], None, rd["condition_search"]["vtr"]),
                ("stream every change", rd["stream_all"]["fst_reader"], fa["stream_all_s"], rd["stream_all"]["vtr"]),
            ]
            for label, a, b, c in rows:
                bs = f"{b * 1000:.2f}" if b is not None else "-"
                L.append(f"| {label} | {a * 1000:.2f} | {bs} | **{c * 1000:.2f}** | {a / c:.1f}x |")
            L.append(f"\nParity check (values/counts identical between wellen and VTR): {'OK' if rd['parity_errors'] == 0 else str(rd['parity_errors']) + ' mismatches'}. Warm random-access value query: {rd['value_at_10x100']['vtr_warm'] * 1e6 / rd['value_at_10x100']['queries']:.1f} us per query.\n")
    tx = results["tx"]
    if tx:
        L.append("## Transaction workloads\n")
        L.append("| workload | transactions | attributes / stages | relations | FTR (LZ4) size | **VTR** size | FTR write | **VTR write** | VTR inline write |")
        L.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
        for r in tx:
            w = r["writers"]
            f, v = w["ftr-lz4"], w["vtr-rust"]
            extra = f.get("attributes", f.get("stages", 0))
            L.append(f"| {r['workload']} | {f['transactions']:,} | {extra:,} | {f.get('relations', 0):,} | {fmt_bytes(f['bytes'])} | **{fmt_bytes(v['bytes'])}** ({v['bytes'] / f['bytes'] * 100:.0f}%) | {f['wall_s']:.3f}s | **{v['wall_s']:.3f}s** ({v['cpu_s']:.2f} cpu) | {w['vtr-rust-inline']['wall_s']:.3f}s |")
        L.append("\nFor the Kanata workloads FTR represents each pipeline stage as a child transaction with a `parent_of` relation (its natural encoding); VTR uses its native stages. Both files carry the same information.\n")
        L.append("\n### Transaction navigation (VTR reader)\n")
        L.append("| workload | open | scan all transactions | 1000 lookups by id | 1000 x relations from+to | window query (1% of run) |")
        L.append("|---|---:|---:|---:|---:|---:|")
        for r in tx:
            d = r["readers"]["vtr"]
            L.append(f"| {r['workload']} | {d['open_s'] * 1000:.2f} ms | {d['scan_all_s']:.3f} s ({d['scanned']:,}) | {d['lookup_1000_s'] * 1000:.1f} ms | {d['relations_1000_s'] * 1000:.1f} ms ({d['relations_found']} found) | {d['window_1pct_s'] * 1000:.2f} ms ({d['window_tx']} tx) |")
    L.append("\n## Workload descriptions\n")
    for r in rtl + tx:
        L.append(f"- **{r['workload']}**: {r['info'].get('description', '')}")
    with open(path, "w") as f:
        f.write("\n".join(L) + "\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("what", choices=["all", "prepare", "run", "report"])
    ap.add_argument("--scale", choices=["small", "full"], default="full")
    ap.add_argument("--out", default=os.path.join(ROOT, "bench", "results", "latest"))
    ap.add_argument("--workloads", default=None)
    ap.add_argument("--repeat", type=int, default=3)
    ap.add_argument("--reads-only", action="store_true", help="re-run only the read benchmarks on the kept output files")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    res_path = os.path.join(a.out, "results.json")
    rtl_names = SMALL_SET if a.scale == "small" else list(RTL_WORKLOADS)
    tx_names = SMALL_TX if a.scale == "small" else list(TX_WORKLOADS)
    if a.workloads:
        sel = a.workloads.split(",")
        rtl_names = [n for n in rtl_names if n in sel]
        tx_names = [n for n in tx_names if n in sel]
    if a.what in ("all", "prepare", "run"):
        build()
    results = {"machine": machine_info(), "rtl": [], "tx": []}
    if os.path.exists(res_path) and a.what in ("report", "run"):
        results = json.load(open(res_path))
        results["machine"] = machine_info() if a.what == "run" else results["machine"]
    if a.what in ("all", "prepare"):
        info = prepare(rtl_names + tx_names, a.scale)
        json.dump(info, open(os.path.join(a.out, "workloads.json"), "w"), indent=2)
    if a.what in ("all", "run"):
        info = prepare(rtl_names + tx_names, a.scale)
        done_rtl = {r["workload"]: r for r in results.get("rtl", [])}
        done_tx = {r["workload"]: r for r in results.get("tx", [])}
        for n in rtl_names:
            if n not in info:
                continue
            t = time.time()
            done_rtl[n] = run_rtl(n, info[n], a.repeat, a.out, a.reads_only, done_rtl.get(n))
            print(f"{n}: done in {time.time() - t:.0f}s", flush=True)
            results["rtl"] = [done_rtl[k] for k in RTL_WORKLOADS if k in done_rtl]
            json.dump(results, open(res_path, "w"), indent=1)
        for n in tx_names:
            if n not in info or a.reads_only:
                continue
            done_tx[n] = run_tx(n, info[n], a.repeat, a.out)
            results["tx"] = [done_tx[k] for k in TX_WORKLOADS if k in done_tx]
            json.dump(results, open(res_path, "w"), indent=1)
    if a.what in ("all", "run", "report"):
        render(results, os.path.join(ROOT, "docs", "BENCHMARK_RESULTS.md"))
        print("report written to docs/BENCHMARK_RESULTS.md")


if __name__ == "__main__":
    main()
