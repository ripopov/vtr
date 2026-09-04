#!/usr/bin/env python3
"""Host-compiler study on the Verilated C910 model.

The Verilated sources of the untraced C910 CoreMark model (bench/workloads/c910,
MODE=none) are compiled with several compilers and option sets, and every binary
runs the same CoreMark iteration pinned to one performance core. Writes
bench/results/latest/compilers.json; `bench/run.py report` renders it.

    python3 bench/compilers.py [--jobs N] [--repeat N] [--only LABEL[,LABEL]]

Needs the C910 model to have been Verilated (python3 bench/run.py prepare
--workloads c910_coremark) and the compilers listed in VARIANTS on PATH; variants
whose compiler is missing are skipped, variants that fail to build are reported.
"""
import argparse
import datetime
import json
import os
import shutil
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GEN = os.path.join(ROOT, "bench", "workloads", "gen", "c910_build")
SRC = os.path.join(GEN, "obj_none")           # Verilated sources of the untraced model
SW = os.path.join(GEN, "sw", "coremark")      # CoreMark image (inst.pat / data.pat)
WORK = os.path.join(GEN, "compilers")
OUT = os.path.join(ROOT, "bench", "results", "latest", "compilers.json")
CORE = "2"  # a performance core of the hybrid host (P-cores 0-7 on this machine)

# Flags Verilator's verilated.mk would set for each compiler family (warnings off).
FAMILY = {
    "gcc": {
        "CFG_CXXFLAGS_NO_UNUSED": "-faligned-new -fcf-protection=none -w",
        "CFG_CXXFLAGS_PCH_I": "-include",
        "CFG_GCH_IF_CLANG": "",
    },
    "clang": {
        "CFG_CXXFLAGS_NO_UNUSED": "-faligned-new -fbracket-depth=4096 -fcf-protection=none -Qunused-arguments -w",
        "CFG_CXXFLAGS_PCH_I": "-include-pch",
        "CFG_GCH_IF_CLANG": ".gch",
    },
}


def variant(label, cxx, opt, extra="", pgo=False, lto=False, note=""):
    fam = "clang" if "clang" in cxx else "gcc"
    return {"label": label, "cxx": cxx, "family": fam, "opt": opt, "extra": extra, "pgo": pgo, "lto": lto, "note": note}


# The matrix: the latest gcc and clang across optimisation levels, with PGO and LTO;
# older versions at -O2 for reference; a few gcc probes (single optimisations off).
VARIANTS = []
for cxx, tag in [("g++-16", "gcc-16"), ("clang++-22", "clang-22")]:
    VARIANTS += [
        variant(f"{tag} -O2", cxx, "-O2"),
        variant(f"{tag} -O3", cxx, "-O3"),
        variant(f"{tag} -O3 -march=native", cxx, "-O3", "-march=native"),
        variant(f"{tag} -O2 PGO", cxx, "-O2", pgo=True),
        variant(f"{tag} -O3 PGO", cxx, "-O3", pgo=True),
        variant(f"{tag} -O3 -march=native PGO", cxx, "-O3", "-march=native", pgo=True),
        variant(f"{tag} -O2 LTO", cxx, "-O2", lto=True),
    ]
VARIANTS += [
    variant("gcc-15 -O2", "g++-15", "-O2"),
    variant("clang-21 -O2", "clang++-21", "-O2"),
    variant("clang-19 -O2", "clang++-19", "-O2"),
    # Ubuntu's gcc enables -fstack-protector-strong, -fstack-clash-protection and
    # _FORTIFY_SOURCE=3 by default; this probe switches them off (clang gets the same flags).
    variant("gcc-16 -O2 no hardening", "g++-16", "-O2", "-fno-stack-protector -fno-stack-clash-protection -U_FORTIFY_SOURCE", note="probe"),
    variant("clang-22 -O2 no hardening", "clang++-22", "-O2", "-fno-stack-protector -fno-stack-clash-protection -U_FORTIFY_SOURCE", note="probe"),
    variant("gcc-16 -O2 no vectorizer", "g++-16", "-O2", "-fno-tree-vectorize -fno-tree-slp-vectorize", note="probe"),
    variant("gcc-16 -O2 no 2nd scheduling", "g++-16", "-O2", "-fno-schedule-insns2", note="probe"),
    variant("gcc-16 -O2 no PRE/GCSE", "g++-16", "-O2", "-fno-gcse -fno-tree-pre", note="probe"),
    variant("gcc-16 -O2 no branch guessing", "g++-16", "-O2", "-fno-guess-branch-probability", note="probe"),
    variant("gcc-16 -O2 simple block order", "g++-16", "-O2", "-freorder-blocks-algorithm=simple", note="probe"),
    variant("gcc-16 -O2 no if-conversion", "g++-16", "-O2", "-fno-if-conversion -fno-if-conversion2 -fno-tree-loop-if-convert", note="probe"),
    variant("gcc-16 -O2 no inlining", "g++-16", "-O2", "-fno-inline-small-functions -fno-inline-functions-called-once -fno-indirect-inlining", note="probe"),
    variant("gcc-16 -O2 big inlining", "g++-16", "-O2", "--param max-inline-insns-auto=200 --param inline-unit-growth=400 --param large-function-growth=400", note="probe"),
    variant("gcc-16 -Os", "g++-16", "-Os", note="probe"),
    variant("gcc-16 -O2 no alignment", "g++-16", "-O2", "-falign-functions=1 -falign-jumps=1 -falign-loops=1 -falign-labels=1", note="probe"),
    variant("clang-22 -Os", "clang++-22", "-Os", note="probe"),
    variant("gcc-16 -O2 source function order", "g++-16", "-O2", "-fno-reorder-functions -fno-toplevel-reorder", note="probe"),
]


def sh(cmd, cwd=None, env=None, check=True, capture=False, log=None):
    if log:
        with open(log, "a") as f:
            f.write("+ " + " ".join(cmd) + "\n")
            r = subprocess.run(cmd, cwd=cwd, env=env, stdout=f, stderr=subprocess.STDOUT, text=True)
    else:
        r = subprocess.run(cmd, cwd=cwd, env=env, capture_output=capture, text=True)
    if check and r.returncode != 0:
        raise RuntimeError(f"command failed ({r.returncode}): {' '.join(cmd)}")
    return r


def fresh_sources(dst):
    """Copies the Verilated sources (no objects) into dst."""
    if os.path.exists(dst):
        shutil.rmtree(dst)
    shutil.copytree(SRC, dst, ignore=shutil.ignore_patterns("*.o", "*.a", "*.d", "*.gch", "Vtop", "*.log", "*.gcda", "*.profraw", "*.profdata"))
    # The generated makefile refers to the harness sources relative to the original
    # object directory; the copies live one level deeper.
    mk = os.path.join(dst, "Vtop.mk")
    c910 = os.path.join(ROOT, "bench", "workloads", "c910")
    text = open(mk).read().replace("../../../c910", c910)
    open(mk, "w").write(text)


def clean_objects(d):
    for f in os.listdir(d):
        if f.endswith((".o", ".a", ".d", ".gch")) or f == "Vtop":
            os.remove(os.path.join(d, f))


def make(d, v, jobs, cpp, ld, log):
    fam = FAMILY[v["family"]]
    cmd = ["make", "-f", "Vtop.mk", f"-j{jobs}", f"CXX={v['cxx']}", f"LINK={v['cxx']}", "OBJCACHE=",
           f"OPT_FAST={v['opt']}", "OPT_SLOW=-O1", f"USER_CPPFLAGS={cpp}", f"USER_LDFLAGS={ld}",
           # mold rejects gcc 16's libatomic linker script; gcc links with its default ld.
           "CFG_LDFLAGS_VERILATED=" + ("-fuse-ld=mold" if v["family"] == "clang" else ""), "Vtop"]
    cmd += [f"{k}={val}" for k, val in fam.items()]
    if v["lto"]:
        cmd.append("AR=" + ("llvm-ar-" + v["cxx"].split("-")[-1] if v["family"] == "clang" else "gcc-ar-" + v["cxx"].split("-")[-1]))
    sh(cmd, cwd=d, log=log)


def train(d, v, log):
    """Runs the instrumented binary on a slice of the workload; returns the profile-use flags."""
    env = dict(os.environ)
    if v["family"] == "clang":
        env["LLVM_PROFILE_FILE"] = os.path.join(d, "train.profraw")
    sh(["taskset", "-c", CORE, os.path.join(d, "Vtop"), "--max-cycles=60000"], cwd=SW, env=env, check=False, log=log)
    if v["family"] == "clang":
        ver = v["cxx"].split("-")[-1]
        sh([f"llvm-profdata-{ver}", "merge", "-o", os.path.join(d, "train.profdata"), os.path.join(d, "train.profraw")], log=log)
        return f"-fprofile-instr-use={os.path.join(d, 'train.profdata')} -Wno-profile-instr-unprofiled -Wno-profile-instr-out-of-date"
    return "-fprofile-use -fprofile-correction -Wno-missing-profile"


def build(v, jobs):
    d = os.path.join(WORK, v["label"].replace(" ", "_").replace("=", ""))
    log = d + ".log"
    if os.path.exists(log):
        os.remove(log)
    fresh_sources(d)
    t = time.time()
    extra = v["extra"]
    ld = ""
    if v["lto"]:
        flto = "-flto=thin" if v["family"] == "clang" else "-flto=auto"
        extra = f"{extra} {flto}".strip()
        ld = flto
    if v["pgo"]:
        gen = "-fprofile-instr-generate" if v["family"] == "clang" else "-fprofile-generate"
        make(d, v, jobs, f"{extra} {gen}".strip(), f"{ld} {gen}".strip(), log)
        use = train(d, v, log)
        clean_objects(d)
        extra = f"{extra} {use}".strip()
    make(d, v, jobs, extra, ld, log)
    return d, time.time() - t


def binary_stats(exe):
    """Text size and instruction mix of the whole executable."""
    r = sh(["size", exe], capture=True)
    text = int(r.stdout.splitlines()[1].split()[0])
    dis = subprocess.run(["objdump", "-d", "--no-show-raw-insn", exe], capture_output=True, text=True).stdout
    insns = 0
    mem = 0
    for line in dis.splitlines():
        if not line.startswith(" "):
            continue
        parts = line.split("\t")
        if len(parts) < 2:
            continue
        insns += 1
        if "(%" in parts[1]:
            mem += 1
    return {"text_bytes": text, "instructions": insns, "memory_operand_insns": mem}


def run(exe, repeat):
    best = None
    for _ in range(repeat):
        r = sh(["taskset", "-c", CORE, exe], cwd=SW, capture=True, check=False)
        line = [l for l in r.stdout.splitlines() if l.startswith("{")]
        if not line:
            raise RuntimeError(f"{exe}: no result line\n{r.stdout[-2000:]}\n{r.stderr[-2000:]}")
        j = json.loads(line[-1])
        if j.get("result") != "PASS":
            raise RuntimeError(f"{exe}: {j}")
        if best is None or j["wall_s"] < best["wall_s"]:
            best = j
    return best


def compiler_version(cxx):
    r = subprocess.run([cxx, "--version"], capture_output=True, text=True)
    return r.stdout.splitlines()[0] if r.returncode == 0 else "n/a"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--jobs", type=int, default=10, help="make jobs per build (two builds run at a time)")
    ap.add_argument("--repeat", type=int, default=2)
    ap.add_argument("--only", default=None)
    a = ap.parse_args()
    if not os.path.exists(os.path.join(SRC, "Vtop.mk")):
        sys.exit("Verilate the C910 model first: python3 bench/run.py prepare --workloads c910_coremark")
    os.makedirs(WORK, exist_ok=True)
    variants = [v for v in VARIANTS if shutil.which(v["cxx"])]
    if a.only:
        sel = a.only.split(",")
        variants = [v for v in variants if v["label"] in sel]
    skipped = [v["label"] for v in VARIANTS if not shutil.which(v["cxx"])]
    results = {"date": datetime.datetime.now().isoformat(timespec="seconds"), "core": CORE, "skipped": skipped,
               "compilers": {v["cxx"]: compiler_version(v["cxx"]) for v in variants}, "variants": []}
    if a.only and os.path.exists(OUT):
        # A partial run updates the selected rows of the previous results and keeps the rest.
        prev = json.load(open(OUT))
        results["variants"] = [r for r in prev["variants"] if r["label"] not in sel]
        results["compilers"] = {**prev.get("compilers", {}), **results["compilers"]}
    # Phase 1: build everything (two builds at a time), phase 2: run on a quiet machine.
    from concurrent.futures import ThreadPoolExecutor
    built = {}
    def do_build(v):
        try:
            d, secs = build(v, a.jobs)
            built[v["label"]] = (d, secs, None)
        except Exception as e:  # noqa: BLE001
            built[v["label"]] = (None, 0.0, str(e))
        print(f"built {v['label']}: {'ok' if built[v['label']][0] else 'FAILED'}", flush=True)
    with ThreadPoolExecutor(max_workers=2) as ex:
        list(ex.map(do_build, variants))
    for v in variants:
        d, secs, err = built[v["label"]]
        row = dict(v, build_s=secs)
        if err:
            row["error"] = err
        else:
            exe = os.path.join(d, "Vtop")
            row.update(binary_stats(exe))
            r = run(exe, a.repeat)
            row.update(wall_s=r["wall_s"], cpu_s=r["cpu_s"], cycles=r["cycles"])
            print(f"{v['label']:36s} {r['wall_s']:7.2f} s  text {row['text_bytes'] / 1e6:6.1f} MB  build {secs:5.0f} s", flush=True)
        results["variants"].append(row)
        order = {v["label"]: i for i, v in enumerate(VARIANTS)}
        results["variants"].sort(key=lambda r: order.get(r["label"], len(order)))
        json.dump(results, open(OUT, "w"), indent=1)
    print(f"written {OUT}")


if __name__ == "__main__":
    main()
