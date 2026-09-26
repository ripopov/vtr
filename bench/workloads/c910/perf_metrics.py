#!/usr/bin/env python3
"""Reference model of the proposed C910 performance counters (docs/c910-perf-counters.html).

The proposal adds integer and real signals under TX.core0.perf that a bound
SystemVerilog monitor would compute every cycle: occupancy of the core's
queues, per-cycle event counts from the PMU wires, bus handshakes and
outstanding reads, and top-down slot accounting at rename. This script
computes the same values after the fact, from a capture that dumped the whole
design (`capture.sh` with signals on), so the proposal's demo shows real
CoreMark behaviour and the monitor's rules can later be checked against it,
as pipeline_replay.py does for the pipeline tracer.

It reads each probe through `vtr value` and `vtr changes` and samples every
rising clock edge T (even time units in this harness) with the value just
before the edge (the dump at T - 1), as a monitor clocked on that edge sees it.

    perf_metrics.py <capture.vtr> <coremark.dis> [--window N]
        [--demo-js out.js [--detail NAME:T0:T1]...] [--vtr path/to/vtr]

It prints run totals and a summary per program phase. --demo-js writes the
literal embedded in the proposal page (the `const DATA = ...` line of its
first script): window sums of every metric over the whole run (N cycles per
window), and every cycle of each --detail range (times in the file's units).
The page's literal was made from the `capture.sh --pipeline` recording with

    perf_metrics.py volna/volna/examples/c910_coremark.vtr \\
        bench/workloads/gen/c910_build/sw/coremark/coremark.dis --demo-js demo.js \\
        --detail list:33282:35330 --detail matrix:190466:192514 --detail state:211458:213506
"""
import argparse
import bisect
import collections
import concurrent.futures
import json
import pathlib
import re
import subprocess
import sys

import numpy as np

ROOT = pathlib.Path(__file__).resolve().parents[3]
CPU = "TOP.top.x_soc.x_cpu_sub_system_axi.x_rv_integration_platform.x_cpu_top."
CORE = CPU + "x_ct_top_0.x_ct_core."
IDU, LSU, IFU, RTU = "x_ct_idu_top.", "x_ct_lsu_top.", "x_ct_ifu_top.", "x_ct_rtu_top."
ROB_RT = RTU + "x_ct_rtu_rob.x_ct_rtu_rob_rt."

# key -> (path, width, reduce): reduce "int" keeps the value, "pop" counts set bits.
PROBES = {}


def probe(key, path, width=1, reduce="int", root=CORE):
    PROBES[key] = (root + path + ("" if width == 1 else f" [{width - 1}:0]"), reduce)


for k in range(3):
    probe(f"rt_vld{k}", ROB_RT + f"retire_inst{k}_vld")
    probe(f"rt_num{k}", ROB_RT + f"retire_inst{k}_num", 2)
probe("rt_pc0", ROB_RT + "retire_inst0_cur_pc", 39)
for k in range(4):
    probe(f"rn{k}", IDU + f"x_ct_idu_ir_ctrl.ctrl_ir_pipedown_inst{k}_vld")
for p in range(8):
    probe(f"iss{p}", IDU + f"x_ct_idu_rf_ctrl.ctrl_rf_pipe{p}_pipedown_vld")
probe("ir_stall", IDU + "x_ct_idu_ir_ctrl.ctrl_ir_stall")
probe("is_stall", IDU + "x_ct_idu_is_ctrl.ctrl_is_dis_stall")
probe("bju_cmplt", "iu_rtu_pipe2_cmplt")
probe("rob_full", IDU + "x_ct_idu_is_ctrl.ctrl_is_rob_full")
probe("iq_full", IDU + "x_ct_idu_is_ctrl.ctrl_is_iq_full")
probe("fe_stall", "ifu_hpcp_frontend_stall")
probe("cancel", "iu_yy_xx_cancel")
probe("rob_flush", RTU + "x_ct_rtu_retire.retire_rob_flush")
probe("bht_mis", "iu_rtu_pipe2_bht_mispred")
probe("jmp_mis", "iu_rtu_pipe2_jmp_mispred")
probe("ic_acc", "ifu_hpcp_icache_access")
probe("ic_miss", "ifu_hpcp_icache_miss")
probe("dc_racc", "lsu_hpcp_cache_read_access")
probe("dc_rmiss", "lsu_hpcp_cache_read_miss")
probe("dc_wacc", "lsu_hpcp_cache_write_access")
probe("dc_wmiss", "lsu_hpcp_cache_write_miss")
probe("rob", RTU + "x_ct_rtu_rob.rob_entry_num", 7)
for q, n in (("aiq0", 8), ("aiq1", 8), ("biq", 12), ("lsiq", 12), ("sdiq", 12)):
    probe(q, IDU + f"x_ct_idu_is_{q}.{q}_entry_vld", n, "pop")
probe("lq", LSU + "x_ct_lsu_lq.lq_entry_vld", 16, "pop")
probe("sq", LSU + "x_ct_lsu_sq.sq_entry_vld", 12, "pop")
probe("wmb", LSU + "x_ct_lsu_wmb.wmb_entry_vld", 8, "pop")
probe("rb", LSU + "x_ct_lsu_rb.rb_entry_vld", 8, "pop")
probe("lfb", LSU + "x_ct_lsu_lfb.lfb_addr_entry_vld", 8, "pop")
probe("ibuf", IFU + "x_ct_ifu_ibuf.entry_vld", 32, "pop")
probe("preg_free", RTU + "x_ct_rtu_pst_preg.dealloc", 96, "pop")
probe("div_state", "x_ct_iu_top.x_ct_iu_div.div_cur_state", 6)
for s in ("biu_pad_arvalid", "pad_biu_arready", "pad_biu_rvalid", "biu_pad_rready", "pad_biu_rlast",
          "biu_pad_awvalid", "pad_biu_awready", "biu_pad_wvalid", "pad_biu_wready"):
    probe(s, s, root=CPU)
probe("arid", "biu_pad_arid", 8, root=CPU)
probe("rid", "pad_biu_rid", 8, root=CPU)
probe("arlen", "biu_pad_arlen", 8, root=CPU)


def parse(text):
    return int(text.replace("x", "0").replace("z", "0"), 2)


def history(vtr, file, path, reduce):
    """(times, values): the value at 0 and every change, reduced to an integer."""
    first = subprocess.run([vtr, "value", file, path, "0"], capture_output=True, text=True)
    if first.returncode != 0:
        sys.exit(f"{path}: {first.stderr.strip()}")
    out = subprocess.run([vtr, "changes", file, path], capture_output=True, text=True, check=True).stdout
    times, values = [0], [parse(first.stdout.split()[-1])]
    for line in out.splitlines():
        t, v = line.split("\t")
        times.append(int(t))
        values.append(parse(v))
    if reduce == "pop":
        values = [v.bit_count() for v in values]
    return np.array(times, dtype=np.int64), values


def sample(hist, edges):
    """Per edge T: the probe's value at T - 1."""
    times, values = hist
    idx = np.searchsorted(times, edges - 1, side="right") - 1
    if max(values) < 1 << 62:
        return np.array(values, dtype=np.int64)[idx]
    return [values[i] for i in idx]


def functions(dis):
    """Sorted (start, name) of the program's symbols, from objdump output."""
    syms = []
    for line in pathlib.Path(dis).read_text().splitlines():
        m = re.match(r"^([0-9a-f]+) <([^>]+)>:$", line)
        if m:
            syms.append((int(m.group(1), 16), m.group(2)))
    return sorted(syms)


# Program phases by function name: CoreMark's four kernels, the setup before
# them and the result report after them (the last instruction retired names
# the phase of every cycle).
KERNELS = [
    ("setup", re.compile(r"_init|^__start|^cpu_0_sp|^after_l2en|^get_seed|^main$|^iterate$")),
    ("report", re.compile(r"printf|_out_char|fputc|_ftoa|_ntoa|_out_|time|sim_end|__exit")),
    ("list", re.compile(r"core_list|core_bench_list|cmp_|calc_func|copy_info")),
    ("matrix", re.compile(r"matrix")),
    ("state", re.compile(r"state")),
    ("crc", re.compile(r"crc")),
]


def kernel_of(name):
    for k, rx in KERNELS:
        if rx.search(name):
            return k
    return "setup"


def metrics(v, n):
    """Per-cycle metric arrays from the sampled probes (all length n)."""
    m = {}
    m["retired"] = sum(v[f"rt_vld{k}"] * v[f"rt_num{k}"] for k in range(3))
    m["rob_entries_retired"] = sum(v[f"rt_vld{k}"] for k in range(3))
    flush = v["cancel"] | v["rob_flush"]
    # IR -> IS moves the pipedown slots only while dispatch is not stalled; a
    # stalled IS re-selects the instructions it holds (as c910_tracer.sv does).
    renamed = sum(v[f"rn{k}"] for k in range(4)) * (1 - v["is_stall"]) * (1 - flush)
    m["renamed"] = renamed
    m["issued"] = sum(v[f"iss{p}"] for p in range(8))
    for p in range(8):
        m[f"iss{p}"] = v[f"iss{p}"]
    m["mispredict"] = v["bju_cmplt"] & (v["bht_mis"] | v["jmp_mis"])
    for key in ("ic_acc", "ic_miss", "dc_racc", "dc_rmiss", "dc_wacc", "dc_wmiss", "rob", "aiq0", "aiq1",
                "biq", "lsiq", "sdiq", "lq", "sq", "wmb", "rb", "lfb", "ibuf", "preg_free", "fe_stall",
                "rob_full", "iq_full"):
        m[key] = v[key]
    m["div_busy"] = (v["div_state"] >> 1) & 1
    # Bus: handshakes, beats (16 bytes each on the 128-bit port) and reads in flight.
    ar = v["biu_pad_arvalid"] & v["pad_biu_arready"]
    # The memory model pulses RVALID and RLAST in reset; a read beat needs a request first.
    r = v["pad_biu_rvalid"] & v["biu_pad_rready"] * (np.cumsum(ar) > 0)
    rlast = r & v["pad_biu_rlast"]
    m["ar"] = ar
    m["r_beats"] = r
    m["w_beats"] = v["biu_pad_wvalid"] & v["pad_biu_wready"]
    # A read counts as outstanding from the cycle after its AR handshake to its last beat.
    m["rd_outstanding"] = np.cumsum(ar) - ar - (np.cumsum(rlast) - rlast)
    # Top-down at rename: 4 slots per cycle leave IR for IS. A cycle after a
    # flush is recovery (bad speculation) until rename delivers again; an empty
    # slot while IR or dispatch is stalled is back-end bound, split into memory when a line
    # fill or bus read is outstanding; any other empty slot is front-end bound.
    recovering = np.zeros(n, dtype=np.int64)
    rec = 0
    for i in range(n):
        if flush[i]:
            rec = 1
        elif renamed[i]:
            rec = 0
        recovering[i] = rec
    empty = 4 - renamed
    blocked = v["ir_stall"] | v["is_stall"]
    stalled = blocked & (1 - recovering)
    mem = ((v["lfb"] > 0) | (v["rb"] > 0) | (m["rd_outstanding"] > 0)).astype(np.int64)
    m["slot_recovery"] = empty * recovering
    m["slot_backend_mem"] = empty * stalled * mem
    m["slot_backend_core"] = empty * stalled * (1 - mem)
    m["slot_frontend"] = empty * (1 - recovering) * (1 - blocked)
    m["slot_used"] = renamed
    return m


def read_latencies(v, edges):
    """(issue edge, cycles to last beat, id) of every bus read, matched in order per AXI id."""
    pending = collections.defaultdict(collections.deque)
    out = []
    ar = v["biu_pad_arvalid"] & v["pad_biu_arready"]
    rl = v["pad_biu_rvalid"] & v["biu_pad_rready"] & v["pad_biu_rlast"] * (np.cumsum(ar) > 0)
    for i in np.nonzero(ar | rl)[0]:
        if rl[i] and pending[int(v["rid"][i]) & 31]:
            j = pending[int(v["rid"][i]) & 31].popleft()
            out.append((int(edges[j]), int(i - j), int(v["arid"][j]) & 31))
        if ar[i]:
            pending[int(v["arid"][i]) & 31].append(i)
    return out


def bus_source(axi_id):
    """The requester behind a read's AXI id (ct_lsu_lfb.v, ct_lsu_rb.v, ct_biu_req_arbiter.v)."""
    if axi_id < 8:
        return "D-cache fill"
    if axi_id in (16, 17):
        return "I-cache fill"
    if axi_id == 25:
        return "D prefetch"
    return "uncached/other"


# One printable character per per-cycle value in the demo literal (JSON-safe: no quote or backslash).
ALPHABET = "".join(c for c in map(chr, range(35, 127)) if c != "\\")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("vtr_file")
    ap.add_argument("dis")
    ap.add_argument("--vtr", default=str(ROOT / "target/release/vtr"))
    ap.add_argument("--window", type=int, default=256)
    ap.add_argument("--demo-js")
    ap.add_argument("--detail", action="append", default=[], metavar="NAME:T0:T1")
    args = ap.parse_args()

    clocks = subprocess.check_output([args.vtr, "clocks", args.vtr_file], text=True)
    m = re.search(r"\[(\d+) \.\. (\d+)\] period (\d+)", clocks)
    first, last, period = map(int, m.groups())
    edges = np.arange(first, last + 1, period, dtype=np.int64)
    n = len(edges)

    with concurrent.futures.ThreadPoolExecutor(8) as pool:
        hist = dict(zip(PROBES, pool.map(lambda kv: history(args.vtr, args.vtr_file, *kv[1]), PROBES.items())))
    v = {k: sample(h, edges) for k, h in hist.items()}
    m = metrics(v, n)
    lat = read_latencies(v, edges)

    syms = functions(args.dis)
    starts = [s for s, _ in syms]
    pcs = np.asarray(v["rt_pc0"], dtype=np.int64) << 1
    kernel = ["setup"] * n
    cur = "setup"
    names = {}
    vld = v["rt_vld0"]
    for i in range(n):
        if vld[i]:
            pc = int(pcs[i])
            if pc not in names:
                j = bisect.bisect_right(starts, pc) - 1
                names[pc] = kernel_of(syms[j][1]) if j >= 0 else "setup"
            cur = names[pc]
        kernel[i] = cur

    cycles = n
    tot = {k: int(np.sum(a)) for k, a in m.items()}
    print(f"cycles {cycles}  instructions {tot['retired']}  ROB entries {tot['rob_entries_retired']}"
          f"  IPC {tot['retired'] / cycles:.3f}")
    print(f"renamed {tot['renamed']}  issued {tot['issued']}  mispredicts {tot['mispredict']}")
    print("issue per pipe " + " ".join(f"p{p}={tot[f'iss{p}']}" for p in range(8)))
    print(f"I$ {tot['ic_acc']} acc {tot['ic_miss']} miss  D$ read {tot['dc_racc']} acc {tot['dc_rmiss']} miss"
          f"  write {tot['dc_wacc']} acc {tot['dc_wmiss']} miss")
    print(f"bus AR {tot['ar']}  R beats {tot['r_beats']}  W beats {tot['w_beats']}"
          f"  max reads in flight {int(np.max(m['rd_outstanding']))}")
    changes = {k: int(np.count_nonzero(np.diff(np.asarray(a)))) for k, a in m.items()}
    for k in ("rob", "aiq0", "aiq1", "biq", "lsiq", "sdiq", "lq", "sq", "wmb", "rb", "lfb", "ibuf", "preg_free",
              "rd_outstanding"):
        print(f"  {k:14} mean {np.mean(m[k]):6.2f}  max {int(np.max(m[k])):3}  changes {changes[k]}")
    if lat:
        ls = np.array([l for _, l, _ in lat])
        # Little's law: mean reads in flight = completion rate x mean latency.
        print(f"bus reads {len(lat)}  latency mean {ls.mean():.1f}  median {np.median(ls):.0f}  max {ls.max()}"
              f"  Little {np.mean(m['rd_outstanding']) * cycles / len(lat):.1f}")
    phases = {}
    ph = np.array(kernel)
    for name in ("setup", "list", "matrix", "state", "crc", "report"):
        sel = ph == name
        c = int(np.count_nonzero(sel))
        s_ = {k: int(np.sum(np.asarray(a)[sel])) for k, a in m.items()}
        slots = 4 * c
        phases[name] = {
            "cycles": c, "retired": s_["retired"], "mispredict": s_["mispredict"], "dc_racc": s_["dc_racc"],
            "dc_rmiss": s_["dc_rmiss"], "ic_miss": s_["ic_miss"], "r_beats": s_["r_beats"],
            "rob_mean": round(s_["rob"] / c, 2), "lq_mean": round(s_["lq"] / c, 2),
            "td": [round(x / slots, 4) for x in (s_["retired"], s_["renamed"] - s_["retired"] + s_["slot_recovery"],
                                                  s_["slot_frontend"], s_["slot_backend_core"],
                                                  s_["slot_backend_mem"])],
            "hist_rob": np.bincount(np.asarray(m["rob"])[sel], minlength=64).tolist(),
        }
        p_ = phases[name]
        print(f"{name:7} cycles {c:6}  IPC {p_['retired'] / c:.2f}  MPKI {1000 * p_['mispredict'] / p_['retired']:5.1f}"
              f"  ROB {p_['rob_mean']:5.1f}  top-down ret/bad/fe/be-core/be-mem "
              + "/".join(f"{x:.2f}" for x in p_["td"]))

    if args.demo_js:
        w = args.window
        # Whole run: per window of w cycles the sum (a rate is sum / w) and, for levels, the maximum.
        sum_keys = ["retired", "renamed", "mispredict", "ic_acc", "ic_miss", "dc_racc", "dc_rmiss", "rob",
                    "aiq0", "aiq1", "biq", "lsiq", "sdiq", "lq", "sq", "lfb", "wmb", "ibuf", "preg_free", "r_beats",
                    "rd_outstanding", "slot_frontend", "slot_backend_mem", "slot_backend_core", "slot_recovery",
                    "div_busy"] + [f"iss{p}" for p in range(8)]
        max_keys = ["rob", "lq", "sq", "lsiq", "lfb", "rd_outstanding"]
        nw = cycles // w
        win = {k: np.asarray(m[k][:nw * w]).reshape(nw, w).sum(axis=1).tolist() for k in sum_keys}
        win.update({k + "_max": np.asarray(m[k][:nw * w]).reshape(nw, w).max(axis=1).tolist() for k in max_keys})
        win_kernel = [collections.Counter(kernel[i * w:(i + 1) * w]).most_common(1)[0][0] for i in range(nw)]
        # Details: every cycle, each series one character per cycle (value + offset in ALPHABET).
        cyc_keys = ["retired", "renamed", "mispredict", "rob", "lq", "sq", "aiq0", "aiq1", "biq", "lsiq", "sdiq", "lfb",
                    "rd_outstanding", "r_beats", "dc_rmiss", "dc_racc", "slot_frontend", "slot_backend_mem",
                    "slot_backend_core", "slot_recovery"] + [f"iss{p}" for p in range(8)]
        details = []
        for spec in args.detail:
            name, t0, t1 = spec.split(":")
            i0, i1 = int(np.searchsorted(edges, int(t0))), int(np.searchsorted(edges, int(t1)))
            series = {k: "".join(ALPHABET[int(x)] for x in m[k][i0:i1]) for k in cyc_keys}
            details.append({"name": name, "from_cycle": i0, "kernel": kernel[i0], "series": series})
        hist_lat = collections.defaultdict(list)
        for _, l, i in lat:
            hist_lat[bus_source(i)].append(l)
        data = {
            "cycles": cycles, "period": period, "first": first, "window": w, "alphabet": ALPHABET,
            "totals": {k: tot[k] for k in ("retired", "rob_entries_retired", "renamed", "issued", "mispredict",
                                           "ic_acc", "ic_miss", "dc_racc", "dc_rmiss", "dc_wacc", "dc_wmiss",
                                           "ar", "r_beats", "w_beats")},
            "changes": {k: changes[k] for k in cyc_keys + ["preg_free", "ibuf", "sdiq", "wmb"]},
            "win": win, "win_kernel": win_kernel, "phases": phases, "details": details,
            "read_latency": {k: np.bincount(ls).tolist() for k, ls in sorted(hist_lat.items())},
            "little_latency": round(float(np.mean(m["rd_outstanding"])) * cycles / len(lat), 3),
        }
        pathlib.Path(args.demo_js).write_text("const DATA = " + json.dumps(data, separators=(",", ":")) + ";\n")
        print(f"wrote {args.demo_js}: {nw} windows of {w} cycles, {len(details)} detail ranges")

if __name__ == "__main__":
    main()
