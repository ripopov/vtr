#!/usr/bin/env python3
"""VTR benchmark orchestrator.

Builds the Rust crates and the C/C++ harnesses, prepares the workloads, runs every
writer/reader comparison and writes raw JSON results plus a Markdown report.

    python3 bench/run.py all              # everything (needs verilator for the RSA256 workload)
    python3 bench/run.py prepare          # only workload preparation
    python3 bench/run.py run              # only measurements (workloads must exist)
    python3 bench/run.py report           # only re-render docs/BENCHMARK_RESULTS.md
    python3 bench/run.py compilers        # host-compiler study on the C910 model (bench/compilers.py)

Options: --scale small|full (default full), --out DIR (default bench/results/latest),
         --workloads NAME[,NAME...] to restrict, --repeat N (default 3, best-of),
         --sim-repeat N (default 2, best-of for the simulator runs).
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
# Verilator with the --trace-vtr backend (integrations/verilator), built by build().
VERILATOR = os.path.join(BUILD, "verilator", "install", "bin", "verilator")
VTR_LIBDIR = os.path.join(BUILD, "vtr-lib")
VTR_INCLUDE = os.path.join(ROOT, "crates", "vtr-capi", "include")
SIM_ENV = dict(os.environ, VERILATOR=VERILATOR, VTR_INCLUDE=VTR_INCLUDE, VTR_LIBDIR=VTR_LIBDIR)
SIM_MODES = ["none", "fst", "vtr"]


def sh(cmd, cwd=ROOT, capture=False, env=None, check=True):
    print("+", " ".join(cmd), flush=True)
    r = subprocess.run(cmd, cwd=cwd, capture_output=capture, text=True, env=env)
    if check and r.returncode != 0:
        if capture:
            print(r.stdout)
            print(r.stderr)
        raise SystemExit(f"command failed: {' '.join(cmd)}")
    return r


# Read benchmarks run with glibc's dynamic mmap threshold disabled: otherwise best-of-N
# timings of a load depend on whether an earlier iteration happened to raise the threshold
# (heap reuse, no page faults) or not, which varies with allocation sizes rather than work.
# The same environment applies to every reader (wellen, fstapi, VTR).
READ_ENV = dict(os.environ, MALLOC_MMAP_THRESHOLD_="33554432", MALLOC_TRIM_THRESHOLD_="536870912")


def json_out(cmd, cwd=ROOT, env=None):
    r = sh(cmd, cwd=cwd, capture=True, env=env)
    out = r.stdout
    start = out.find("{")
    return json.loads(out[start:])


def build():
    sh(["cargo", "build", "--release"])
    os.makedirs(BUILD, exist_ok=True)
    sh(["cmake", os.path.join(ROOT, "bench", "cpp"), "-DCMAKE_BUILD_TYPE=Release"], cwd=BUILD)
    sh(["make", "-j8"], cwd=BUILD)
    # The Verilated models link the static VTR C library from a directory of its own.
    os.makedirs(VTR_LIBDIR, exist_ok=True)
    shutil.copy(os.path.join(TARGET, "libvtr.a"), VTR_LIBDIR)
    if not os.path.exists(VERILATOR):
        sh([os.path.join(ROOT, "integrations", "verilator", "build.sh"), os.path.dirname(os.path.dirname(VERILATOR))])


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
    "c910_coremark": ("c910", "1",
                      "pulp-c910 (T-Head openC910: 3-wide superscalar out-of-order RISC-V core with L1 caches, MMU and AXI SoC from ext/pulp-c910) "
                      "running one CoreMark iteration under Verilator, every signal of the design traced (real RTL, large design)"),
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
                # RSA256: one Verilated model per trace mode (none / FST / VTR).
                out = os.path.join(WORK, "rsa256_build")
                exes = {}
                for mode in SIM_MODES:
                    exes[mode] = os.path.join(out, f"obj_{mode}", "rsa_tb")
                    if not os.path.exists(exes[mode]):
                        sh([os.path.join(ROOT, "bench", "workloads", "rsa256", "build.sh"), out, mode], env=SIM_ENV)
                fst = os.path.join(WORK, f"{name}.fst")
                if not os.path.exists(fst):
                    sh([exes["fst"], f"--dump={fst}", f"--cycles={arg}"])
                if not os.path.exists(rpl):
                    sh([VTR_BENCH, "prepare-fst", fst, rpl])
                info[name] = {"source_fst": fst, "cycles": int(arg),
                              "sim": {"exe": exes, "args": [f"--cycles={arg}"], "cwd": out}}
            elif kind == "c910":
                # pulp-c910 + CoreMark: the models are built by bench/workloads/c910/Makefile.
                out = os.path.join(WORK, "c910_build")
                mk = os.path.join(ROOT, "bench", "workloads", "c910")
                sw = os.path.join(out, "sw", "coremark")
                if not os.path.exists(os.path.join(sw, "inst.pat")):
                    sh(["make", "-C", mk, "sw", f"BUILD={out}", f"ITERATIONS={arg}"])
                exes = {}
                for mode in SIM_MODES:
                    exes[mode] = os.path.join(out, f"obj_{mode}", "Vtop")
                    if not os.path.exists(exes[mode]):
                        sh(["make", "-C", mk, "model", f"MODE={mode}", f"BUILD={out}", f"VERILATOR={VERILATOR}",
                            f"VTR_INCLUDE={VTR_INCLUDE}", f"VTR_LIBDIR={VTR_LIBDIR}"])
                fst = os.path.join(WORK, f"{name}.fst")
                if not os.path.exists(fst):
                    sh([exes["fst"], f"--dump={fst}"], cwd=sw)
                if not os.path.exists(rpl):
                    sh([VTR_BENCH, "prepare-fst", fst, rpl])
                info[name] = {"source_fst": fst, "coremark_iterations": int(arg),
                              "sim": {"exe": exes, "args": [], "cwd": sw}}
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


def run_sim(name, sim, repeat, out_dir):
    """Runs the Verilated model without tracing and with a full FST / VTR dump (best of N)."""
    res = {}
    for mode in SIM_MODES:
        dump = os.path.join(out_dir, f"{name}_sim.{mode}")
        cmd = [sim["exe"][mode]] + sim["args"] + ([f"--dump={dump}"] if mode != "none" else [])
        r = best_of(repeat, lambda: json_out(cmd, cwd=sim["cwd"]))
        res[mode] = r
        print(f"  {name} sim {mode:5s} {r['wall_s']:.3f}s wall {r['cpu_s']:.3f}s cpu {r['bytes']:>12} bytes", flush=True)
    return res


# Replays above this many changes take minutes per writer; they are measured once.
SINGLE_RUN_CHANGES = 200_000_000


def vcd_check(name, info, vtr_file, out_dir):
    """Converts the source FST and the VTR files to VCD and compares the value-change counts.

    The FST side uses GTKWave's own fst2vcd when it is installed (an implementation
    independent of every library in this repository), else `vtr fst-to-vcd`; the VTR
    side uses vtr2vcd. Counts must agree in total and per signal for the replay-written
    VTR and, when present, for the VTR written by the simulator itself."""
    fst_vcd = os.path.join(out_dir, f"{name}_fst.vcd")
    tool = shutil.which("fst2vcd")
    t = time.time()
    if tool:
        sh([tool, "-f", info["source_fst"], "-o", fst_vcd])
    else:
        sh([VTR_CLI, "fst-to-vcd", info["source_fst"], fst_vcd])
    res = {"fst_tool": "gtkwave fst2vcd" if tool else "vtr fst-to-vcd", "fst_to_vcd_s": time.time() - t, "files": {}}
    pairs = [("vtr-rust", vtr_file)]
    sim_vtr = os.path.join(out_dir, f"{name}_sim.vtr")
    if os.path.exists(sim_vtr):
        pairs.append(("verilator-vtr", sim_vtr))
    for label, path in pairs:
        vcd = os.path.join(out_dir, f"{name}_{label}.vcd")
        conv = json_out([os.path.join(TARGET, "vtr2vcd"), path, vcd])
        r = sh([VTR_CLI, "vcd-compare", fst_vcd, vcd], capture=True, check=False)
        cmp_ = json.loads(r.stdout[r.stdout.find("{"):])
        cmp_["vtr2vcd_s"] = conv["wall_s"]
        res["files"][label] = cmp_
        print(f"  {name} vcd-check {label:14s} fst {cmp_['changes_a']:,} vtr {cmp_['changes_b']:,} changes, "
              f"{cmp_['mismatched_signals']} signals differ -> {'identical' if cmp_['identical'] else 'DIFFERENT'}", flush=True)
        os.remove(vcd)
    os.remove(fst_vcd)
    res["identical"] = all(f["identical"] for f in res["files"].values())
    return res


def run_rtl(name, info, repeat, out_dir, reads_only=False, previous=None, sim_repeat=2):
    rpl = os.path.join(WORK, f"{name}.rpl")
    res = {"workload": name, "info": info, "writers": {}, "readers": {}}
    if info.get("changes", 0) > SINGLE_RUN_CHANGES:
        repeat = 1
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
    # Uncompressed variants of every writer.
    w("fstapi-none", [fw, rpl, os.path.join(out_dir, f"{name}_fstapi_none.fst"), "fstapi", "none"], os.path.join(out_dir, f"{name}_fstapi_none.fst"))
    w("fstcpp-none", [fw, rpl, os.path.join(out_dir, f"{name}_fstcpp_none.fst"), "fstcpp", "none"], os.path.join(out_dir, f"{name}_fstcpp_none.fst"))
    w("vtr-none", [VTR_BENCH, "write", rpl, os.path.join(out_dir, f"{name}_none.vtr"), "--codec", "none"], os.path.join(out_dir, f"{name}_none.vtr"))
    # Simulator-integrated: the Verilated model itself writing FST (Verilator's built-in writer)
    # or VTR (--trace-vtr), against the same model with tracing off.
    if "sim" in info:
        if reads_only and previous is not None and "simulator" in previous:
            res["simulator"] = previous["simulator"]
        else:
            res["simulator"] = run_sim(name, info["sim"], sim_repeat, out_dir)
    # Reads: wellen (what wavepeek uses) vs VTR, on the GTKWave-default FST (zlib) and, when the
    # workload came from a simulator, on the original simulator-written FST as well.
    vtr_file = files["vtr-rust"]
    res["readers"]["vs_fstapi_zlib"] = json_out([VTR_BENCH, "read", files["fstapi-zlib"], vtr_file], env=READ_ENV)
    if "source_fst" in info:
        res["readers"]["vs_source_fst"] = json_out([VTR_BENCH, "read", info["source_fst"], vtr_file], env=READ_ENV)
    sim_fst, sim_vtr = os.path.join(out_dir, f"{name}_sim.fst"), os.path.join(out_dir, f"{name}_sim.vtr")
    if os.path.exists(sim_fst) and os.path.exists(sim_vtr):
        # Both files written by the simulator: checks the Verilator backend (parity) and
        # compares reads of what each simulator run actually produced.
        res["readers"]["vs_sim"] = json_out([VTR_BENCH, "read", sim_fst, sim_vtr], env=READ_ENV)
    if not reads_only or (os.path.exists(files["fstcpp-none"]) and os.path.exists(files["vtr-none"])):
        res["readers"]["uncompressed"] = json_out([VTR_BENCH, "read", files["fstcpp-none"], files["vtr-none"]], env=READ_ENV)
    plan = os.path.join(out_dir, f"{name}_plan.txt")
    with open(plan, "w") as f:
        f.write(sh([VTR_BENCH, "plan", vtr_file], capture=True).stdout)
    res["readers"]["fstapi_reader_zlib"] = json_out([os.path.join(BUILD, "fst_read"), files["fstapi-zlib"], plan], env=READ_ENV)
    res["readers"]["fstapi_reader_lz4"] = json_out([os.path.join(BUILD, "fst_read"), files["fstapi-lz4"], plan], env=READ_ENV)
    # Sizes of the original simulator FST for reference.
    if "source_fst" in info:
        res["source_fst_bytes"] = os.path.getsize(info["source_fst"])
        # Information check: FST and VTR converted to VCD must hold the same changes.
        res["vcd_check"] = vcd_check(name, info, vtr_file, out_dir)
    # Remove bulky outputs but keep one VTR and the zlib FST for inspection (and reads-only reruns).
    for label, path in files.items():
        if label not in ("vtr-rust", "fstapi-zlib", "fstcpp-none", "vtr-none"):
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


def model_cxx():
    """Host compiler the Verilated models are built with (CFG_CXX_VERSION of the installed verilated.mk)."""
    mk = os.path.join(os.path.dirname(os.path.dirname(VERILATOR)), "share", "verilator", "include", "verilated.mk")
    try:
        for line in open(mk):
            if line.startswith("CFG_CXX_VERSION"):
                return line.split("=", 1)[1].strip().strip('"')
    except OSError:
        pass
    return "n/a"


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
        "rustc": ver(["rustc", "--version"]), "gcc": ver(["gcc", "--version"]),
        "verilator": ver([VERILATOR if os.path.exists(VERILATOR) else "verilator", "--version"]),
        "model_cxx": model_cxx(),
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


def summary(rtl, tx):
    """Claim-by-claim summary computed from the raw results."""
    L = ["## Summary\n"]
    if rtl:
        size_ratios = []
        write_ratios = []
        inline_ratios = []
        for r in rtl:
            w = r["writers"]
            best_fst = min(w["fstapi-lz4"]["bytes"], w["fstapi-zlib"]["bytes"], w["fstcpp-lz4"]["bytes"])
            size_ratios.append((r["workload"], w["vtr-rust"]["bytes"] / best_fst))
            fastest = min(w["fstapi-lz4"]["wall_s"], w["fstapi-zlib"]["wall_s"], w["fstcpp-lz4"]["wall_s"])
            write_ratios.append((r["workload"], fastest / w["vtr-rust"]["wall_s"]))
            inline_ratios.append((r["workload"], fastest / w["vtr-rust-inline"]["wall_s"]))
        wins = sum(1 for _, x in size_ratios if x < 1)
        L.append(f"- **Size.** VTR (zstd-3) is smaller than the smallest FST variant on {wins} of {len(rtl)} signal workloads: "
                 + ", ".join(f"{n} {x * 100:.0f}%" for n, x in size_ratios) + " of the best FST size.")
        wins = sum(1 for _, x in write_ratios if x > 1)
        L.append(f"- **Write speed.** VTR (background encoder) is faster than the fastest FST writer on {wins} of {len(rtl)} workloads: "
                 + ", ".join(f"{n} {x:.2f}x" for n, x in write_ratios) + ". Inline encoder: "
                 + ", ".join(f"{x:.2f}x" for _, x in inline_ratios) + ".")
        keys = [("open", "open"), ("load_1", "load 1 signal"), ("load_10", "load 10"), ("load_100", "load 100"), ("load_1000", "load 1000"),
                ("value_at_10x100", "value at time"), ("changes_window_1pct", "window scan"), ("condition_search", "condition search"), ("stream_all", "stream all")]
        rows = []
        losses = []
        for k, label in keys:
            ratios = []
            for r in rtl:
                rd = r["readers"]["vs_fstapi_zlib"]
                base = rd[k].get("fst_wellen", rd[k].get("fst_reader"))
                x = base / rd[k]["vtr"]
                ratios.append(x)
                if x < 1:
                    losses.append(f"{label} on {r['workload']} ({x:.2f}x)")
            rows.append(f"{label} {min(ratios):.1f}-{max(ratios):.1f}x")
        L.append("- **Read and navigation** (VTR speed-up over wellen, min-max across workloads; sub-millisecond queries are noisy): " + ", ".join(rows) + ". "
                 + ("VTR is slower in: " + "; ".join(losses) + "." if losses else "VTR is faster in every query on every workload."))
        parity = all(r["readers"]["vs_fstapi_zlib"]["parity_errors"] == 0 for r in rtl)
        L.append(f"- **Parity.** Values and change counts identical between wellen and VTR on all workloads: {'yes' if parity else 'NO'}.")
        checks = [r for r in rtl if "vcd_check" in r]
        if checks:
            ok = all(r["vcd_check"]["identical"] for r in checks)
            L.append("- **Information check.** Source FST and VTR files converted to VCD carry the same value changes, in total and per signal, on "
                     + ", ".join(f"{r['workload']} ({r['vcd_check']['files']['vtr-rust']['changes_a']:,} changes)" for r in checks)
                     + f": {'yes' if ok else 'NO'}.")
        sims = [r for r in rtl if "simulator" in r]
        if sims:
            parts = []
            for r in sims:
                s_ = r["simulator"]
                base, f, v = s_["none"]["wall_s"], s_["fst"], s_["vtr"]
                cf, cv = f["wall_s"] - base, v["wall_s"] - base
                parts.append(f"{r['workload']}: tracing adds {cf:.2f} s with FST and {cv:.2f} s with VTR to a {base:.2f} s simulation "
                             f"({cf / max(cv, 1e-9):.2f}x less trace cost), VTR file {v['bytes'] / f['bytes'] * 100:.0f}% of Verilator's FST")
            sim_parity = all(r["readers"].get("vs_sim", {}).get("parity_errors", 0) == 0 for r in sims)
            L.append("- **Simulator-integrated (Verilator --trace-fst vs --trace-vtr, whole design traced).** " + "; ".join(parts)
                     + f". Files written by the two Verilator backends read back identically: {'yes' if sim_parity else 'NO'}.")
    if tx:
        parts = []
        for r in tx:
            w = r["writers"]
            f, v = w["ftr-lz4"], w["vtr-rust"]
            parts.append(f"{r['workload']} size {v['bytes'] / f['bytes'] * 100:.0f}% of FTR-LZ4, write speed {f['wall_s'] / v['wall_s']:.2f}x FTR-LZ4"
                         + (f" and {w['ftr-raw']['wall_s'] / v['wall_s']:.2f}x uncompressed FTR" if "ftr-raw" in w else ""))
        L.append("- **Transactions.** " + "; ".join(parts) + ".")
    L.append("")
    return L


def render(results, path):
    m = results["machine"]
    L = []
    L.append("# VTR benchmark results\n")
    L.append("Generated by `bench/run.py`; raw data in `bench/results/latest/results.json`. See `docs/BENCHMARKS.md` for the methodology.\n")
    L.append(f"Machine: {m['cpu']} ({m['cores']} threads), {m['memory']}, Linux {m['kernel']}; {m['rustc']}; {m['gcc']}; verilator: {m['verilator']} (models compiled with {m.get('model_cxx', 'n/a')}). Run on {m['date']}.\n")
    L.append("All timings are best-of-N wall-clock seconds of the write loop plus close (writers) or of the query (readers); `cpu` is user+system CPU time of the whole process, including VTR's background encoder thread.\n")
    rtl = [r for r in results["rtl"]]
    tx = results["tx"]
    L.extend(summary(rtl, tx))
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
    sims = [r for r in rtl if "simulator" in r]
    if sims:
        L.append("## Simulator-integrated tracing: Verilator with --trace-fst versus --trace-vtr\n")
        L.append("The Verilated model itself writes the trace (every signal of the design, dumped after each clock edge) through Verilator's built-in FST backend (libfstwriter, LZ4) or through the VTR backend from `integrations/verilator`; the same model built without tracing gives the baseline. Best of N whole-process runs; `cpu` includes VTR's background encoder.\n")
        L.append("| workload | cycles | no trace | Verilator FST | Verilator **VTR** | trace cost FST / VTR | FST size | **VTR** size | VTR vs FST |")
        L.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
        for r in sims:
            s_ = r["simulator"]
            n, f, v = s_["none"], s_["fst"], s_["vtr"]
            cyc = f"{n.get('cycles', 0):,}"
            L.append(f"| {r['workload']} | {cyc} | {n['wall_s']:.2f}s | {f['wall_s']:.2f}s ({f['cpu_s']:.2f} cpu) | **{v['wall_s']:.2f}s** ({v['cpu_s']:.2f} cpu) | "
                     f"+{f['wall_s'] - n['wall_s']:.2f}s / **+{v['wall_s'] - n['wall_s']:.2f}s** ({(f['wall_s'] - n['wall_s']) / max(v['wall_s'] - n['wall_s'], 1e-9):.2f}x) | "
                     f"{fmt_bytes(f['bytes'])} | **{fmt_bytes(v['bytes'])}** | {v['bytes'] / f['bytes'] * 100:.0f}% |")
        L.append("\nReads of the simulator-written files (wellen on Verilator's FST, VTR on Verilator's VTR; milliseconds):\n")
        L.append("| workload | open | load 1 | load 100 | load 1000 | value at time 10x100 | window scan | stream all | parity |")
        L.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
        for r in sims:
            rd = r["readers"].get("vs_sim")
            if not rd:
                continue
            def rr(k):
                base = rd[k].get("fst_wellen", rd[k].get("fst_reader"))
                return f"{base * 1000:.1f} / **{rd[k]['vtr'] * 1000:.1f}**"
            L.append(f"| {r['workload']} | {rr('open')} | {rr('load_1')} | {rr('load_100')} | {rr('load_1000')} | {rr('value_at_10x100')} | {rr('changes_window_1pct')} | {rr('stream_all')} | {'OK' if rd['parity_errors'] == 0 else str(rd['parity_errors']) + ' mismatches'} |")
        L.append("")
    checks = [r for r in rtl if "vcd_check" in r]
    if checks:
        L.append("## Information check: FST and VTR converted to VCD\n")
        L.append("The simulator's FST is converted to VCD (GTKWave's `fst2vcd` when installed, else `vtr fst-to-vcd`) and so are the VTR files (`vtr2vcd`); `vtr vcd-compare` then counts the value changes of both VCDs in total and per signal. *replay VTR* is the file the benchmark writer produced from the replay of the FST; *Verilator VTR* is the file the simulator wrote through `--trace-vtr`.\n")
        L.append("| workload | FST changes | replay VTR changes | Verilator VTR changes | signals | per-signal identical | fst2vcd | vtr2vcd |")
        L.append("|---|---:|---:|---:|---:|---:|---:|---:|")
        for r in checks:
            c = r["vcd_check"]
            f = c["files"]
            a = f["vtr-rust"]
            sim = f.get("verilator-vtr")
            ident = all(x["identical"] for x in f.values())
            L.append(f"| {r['workload']} | {a['changes_a']:,} | {a['changes_b']:,} | {sim['changes_b']:,} | {a['signals_a']:,} | "
                     f"{'yes' if ident else 'NO (' + str(sum(x['mismatched_signals'] for x in f.values())) + ' signals)'} | "
                     f"{c['fst_to_vcd_s']:.1f}s ({c['fst_tool']}) | {a['vtr2vcd_s']:.1f}s |" if sim else
                     f"| {r['workload']} | {a['changes_a']:,} | {a['changes_b']:,} | - | {a['signals_a']:,} | "
                     f"{'yes' if ident else 'NO (' + str(a['mismatched_signals']) + ' signals)'} | {c['fst_to_vcd_s']:.1f}s ({c['fst_tool']}) | {a['vtr2vcd_s']:.1f}s |")
        L.append("")
    if rtl and any("uncompressed" in r["readers"] for r in rtl):
        L.append("## Uncompressed variants: VTR (codec none) versus FST without value compression\n")
        L.append("`fstapi none` = pack type FASTLZ in the vendored build, where fastlz is disabled and every value chain is stored raw (time tables and frames remain zlib-packed); `libfstwriter none` = `NO_COMPRESSION` (no compression anywhere). VTR `none` stores every blob raw. Reads compare wellen on the libfstwriter file with VTR on the uncompressed VTR file.\n")
        L.append("| workload | fstapi none | libfstwriter none | **VTR none** | VTR vs smallest | fstapi none write | libfstwriter none write | **VTR none write** | open | load 1000 | value at time 10x100 | stream all |")
        L.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
        for r in rtl:
            w = r["writers"]
            if "vtr-none" not in w:
                continue
            best = min(w["fstapi-none"]["bytes"], w["fstcpp-none"]["bytes"])
            rd = r["readers"].get("uncompressed")
            def rr(k):
                if not rd:
                    return "-"
                base = rd[k].get("fst_wellen", rd[k].get("fst_reader"))
                return f"{base * 1000:.1f} / **{rd[k]['vtr'] * 1000:.1f}** ms"
            L.append(f"| {r['workload']} | {fmt_bytes(w['fstapi-none']['bytes'])} | {fmt_bytes(w['fstcpp-none']['bytes'])} | **{fmt_bytes(w['vtr-none']['bytes'])}** | {w['vtr-none']['bytes'] / best * 100:.0f}% | {w['fstapi-none']['wall_s']:.3f}s | {w['fstcpp-none']['wall_s']:.3f}s | **{w['vtr-none']['wall_s']:.3f}s** | {rr('open')} | {rr('load_1000')} | {rr('value_at_10x100')} | {rr('stream_all')} |")
        L.append("\nRead cells are wellen / **VTR** milliseconds.\n")
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
    comp = os.path.join(ROOT, "bench", "results", "latest", "compilers.json")
    if os.path.exists(comp):
        L.extend(render_compilers(json.load(open(comp))))
    L.append("\n## Workload descriptions\n")
    for r in rtl + tx:
        L.append(f"- **{r['workload']}**: {r['info'].get('description', '')}")
    with open(path, "w") as f:
        f.write("\n".join(L) + "\n")


def render_compilers(c):
    """Section for the host-compiler study (bench/compilers.py)."""
    L = ["## Host compiler: gcc versus clang on the Verilated C910 model\n"]
    L.append("The Verilated sources of the untraced C910 CoreMark model, compiled by `bench/compilers.py` with different compilers and options, each binary running the same CoreMark iteration pinned to one performance core (best of N). "
             "`OPT_FAST` is the flag shown, `OPT_SLOW` is `-O1`, the runtime (`verilated.cpp`) keeps Verilator's default; PGO trains on the first 60k cycles of the same run; LTO is `-flto=thin` (clang) / `-flto=auto` (gcc). "
             "`memory ops` is the share of instructions in the executable with a memory operand.\n")
    L.append("Compilers: " + "; ".join(f"`{k}` = {v}" for k, v in c["compilers"].items()) + f". Run on {c['date']}.\n")
    L.append("| variant | CoreMark run | vs fastest | text size | instructions | memory ops | build |")
    L.append("|---|---:|---:|---:|---:|---:|---:|")
    ok = [v for v in c["variants"] if "wall_s" in v]
    best = min(v["wall_s"] for v in ok) if ok else 1.0
    for v in c["variants"]:
        if "wall_s" not in v:
            L.append(f"| {v['label']} | build failed | | | | | |")
            continue
        L.append(f"| {v['label']}{' (probe)' if v.get('note') == 'probe' else ''} | {v['wall_s']:.2f} s | {v['wall_s'] / best:.2f}x | {v['text_bytes'] / 1e6:.1f} MB | "
                 f"{v['instructions'] / 1e6:.2f} M | {v['memory_operand_insns'] / max(v['instructions'], 1) * 100:.0f}% | {v['build_s']:.0f} s |")
    if c.get("skipped"):
        L.append(f"\nSkipped (compiler not installed): {', '.join(c['skipped'])}.")
    L.append("")
    return L


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("what", choices=["all", "prepare", "run", "report", "compilers"])
    ap.add_argument("--scale", choices=["small", "full"], default="full")
    ap.add_argument("--out", default=os.path.join(ROOT, "bench", "results", "latest"))
    ap.add_argument("--workloads", default=None)
    ap.add_argument("--repeat", type=int, default=3)
    ap.add_argument("--sim-repeat", type=int, default=2, help="best-of N for the simulator runs (minutes each on c910)")
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
    if a.what == "compilers":
        import compilers
        sys.argv = [sys.argv[0]]
        compilers.main()
        render(json.load(open(res_path)), os.path.join(ROOT, "docs", "BENCHMARK_RESULTS.md"))
        return
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
            done_rtl[n] = run_rtl(n, info[n], a.repeat, a.out, a.reads_only, done_rtl.get(n), a.sim_repeat)
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
