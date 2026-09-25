#!/usr/bin/env python3
"""Offline replay of the proposed C910 pipeline tracer over a whole-design recording.

docs/c910-verilator-tx-stream.html proposes a SystemVerilog tracer that records
every C910 instruction as a VTR transaction. This script runs the same identity
logic after the fact, from the probe signals of a recording made with
`capture.sh` (or the benchmark's c910_coremark.vtr), so the probe map can be
checked before any simulator work and later compared with the live tracer.

It reads only the probe signals, through `vtr value` and `vtr changes`, walks
the rising clock edges (even time units in this harness) and applies, per edge:
retire, ROB flush, complete, LSU stages, RF, dispatch (ROB create binds the next
inst_num front-end instructions to one IID), front-end flush, IR->IS, ID->IR,
IB->ID. It then checks the result against the core's own retire signals:
retire PCs, program order of the retired stream.

    pipeline_replay.py <file.vtr> <coremark.dis> --from T0 --to T1
        [--json out.json] [--demo-js out.js] [--vtr path/to/vtr]

Known shortcuts (see the proposal): folded ROB members are matched in program
order rather than by destination register, multiply/divide complete at
selection, the store-data pipe and the fetch stages are not reconstructed.
"""
import argparse
import json
import pathlib
import re
import subprocess
import sys
from collections import deque

ROOT = pathlib.Path(__file__).resolve().parents[3]
CORE = "TOP.top.x_soc.x_cpu_sub_system_axi.x_rv_integration_platform.x_cpu_top.x_ct_top_0.x_ct_core."
IDU = "x_ct_idu_top."
LSU = "x_ct_lsu_top."
RT = "x_ct_rtu_top.x_ct_rtu_rob.x_ct_rtu_rob_rt."
WARMUP = 400  # time units replayed before --from so in-flight state is known

SIG = {}


def sig(key, path, width=1):
    SIG[key] = path + ("" if width == 1 else f" [{width - 1}:0]")


sig("cancel", "iu_yy_xx_cancel")
sig("flush_fe", "rtu_idu_flush_fe")
sig("rob_flush", "x_ct_rtu_top.x_ct_rtu_retire.retire_rob_flush")
sig("id_stall", IDU + "x_ct_idu_id_ctrl.ctrl_id_pipedown_stall")
sig("ir_stall", IDU + "x_ct_idu_ir_ctrl.ctrl_ir_stall")
sig("is_stall", IDU + "x_ct_idu_is_ctrl.ctrl_is_dis_stall")
for k in range(3):
    sig(f"id_vld{k}", IDU + f"x_ct_idu_id_ctrl.id_inst{k}_vld")
    sig(f"id_data{k}", IDU + f"x_ct_idu_id_dp.id_inst{k}_data", 73)
for k in range(4):
    sig(f"id_pd{k}", IDU + f"x_ct_idu_ir_ctrl.ctrl_id_pipedown_inst{k}_vld")
    sig(f"ir_data{k}", IDU + f"x_ct_idu_ir_dp.ir_inst{k}_data", 178)
    sig(f"ir_pd{k}", IDU + f"x_ct_idu_is_ctrl.ctrl_ir_pipedown_inst{k}_vld")
    sig(f"create_en{k}", IDU + f"x_ct_idu_is_ctrl.idu_rtu_rob_create{k}_en")
    sig(f"create_data{k}", f"idu_rtu_rob_create{k}_data", 40)
    sig(f"create_iid{k}", f"rtu_idu_rob_inst{k}_iid", 7)
PIPE_IID = {0: "idu_iu_rf_pipe0_iid", 1: "idu_iu_rf_pipe1_iid", 2: "idu_iu_rf_pipe2_iid",
            3: "idu_lsu_rf_pipe3_iid", 4: "idu_lsu_rf_pipe4_iid", 6: "idu_vfpu_rf_pipe6_iid",
            7: "idu_vfpu_rf_pipe7_iid"}
PIPE_STAGE = {0: "EX", 1: "EX", 2: "BJ", 6: "FX", 7: "FX"}
for n, path in PIPE_IID.items():
    sig(f"rf_vld{n}", IDU + f"x_ct_idu_rf_ctrl.rf_pipe{n}_inst_vld")
    sig(f"rf_fail{n}", IDU + f"x_ct_idu_rf_ctrl.ctrl_rf_pipe{n}_lch_fail")
    sig(f"rf_iid{n}", path, 7)
CMPLT = {0: "iu_rtu_pipe0", 1: "iu_rtu_pipe1", 2: "iu_rtu_pipe2", 3: "lsu_rtu_wb_pipe3",
         4: "lsu_rtu_wb_pipe4", 6: "vfpu_rtu_pipe6", 7: "vfpu_rtu_pipe7"}
for n, base in CMPLT.items():
    sig(f"cm{n}", base + "_cmplt")
    sig(f"cm_iid{n}", base + "_iid", 7)
LSU_STAGES = [("AG", "ld_ag", "x_ct_lsu_ld_ag"), ("DC", "ld_dc", "x_ct_lsu_ld_dc"),
              ("DA", "ld_da", "x_ct_lsu_ld_da"), ("WB", "ld_wb", "x_ct_lsu_ld_wb"),
              ("AG", "st_ag", "x_ct_lsu_st_ag"), ("DC", "st_dc", "x_ct_lsu_st_dc"),
              ("DA", "st_da", "x_ct_lsu_st_da")]
for _, s, mod in LSU_STAGES:
    sig(f"{s}_vld", LSU + f"{mod}.{s}_inst_vld")
    sig(f"{s}_iid", LSU + f"{mod}.{s}_iid", 7)
for k in range(3):
    sig(f"rt_vld{k}", RT + f"retire_inst{k}_vld")
    sig(f"rt_iid{k}", RT + f"retire_inst{k}_iid", 7)
sig("rt_pc0", RT + "retire_inst0_cur_pc", 39)
sig("rt_pc1", RT + "rob_retire_inst1_cur_pc", 39)
sig("rt_pc2", RT + "rob_retire_inst2_cur_pc", 39)


def bin_value(text):
    return int(text.replace("x", "0").replace("z", "0"), 2)


def history(vtr, path, t0, t1):
    """[(time, value)] from the value at t0 through every change up to t1."""
    name = CORE + path
    first = subprocess.run([vtr, "value", VTR_FILE, name, str(t0)], capture_output=True, text=True)
    if first.returncode != 0:
        sys.exit(f"{name}: {first.stderr.strip()}")
    out = [(t0, bin_value(first.stdout.split()[-1]))]
    changes = subprocess.run([vtr, "changes", VTR_FILE, name, "--from", str(t0), "--to", str(t1)],
                             capture_output=True, text=True, check=True)
    for line in changes.stdout.splitlines():
        t, v = line.split("\t")
        out.append((int(t), bin_value(v)))
    return out


def samples(hist, times):
    """Per time: {key: value}, sampling each step function at non-decreasing times."""
    idx = {k: 0 for k in hist}
    for t in times:
        row = {}
        for k, ch in hist.items():
            i = idx[k]
            while i + 1 < len(ch) and ch[i + 1][0] <= t:
                i += 1
            idx[k] = i
            row[k] = ch[i][1]
        yield t, row


def bits(v, hi, lo):
    return (v >> lo) & ((1 << (hi - lo + 1)) - 1)


class Insn:
    count = 0

    def __init__(self, pc, op, t, stage):
        Insn.count += 1
        self.seq = Insn.count
        self.pc, self.op, self.begin = pc, op, t
        self.stages = [[stage, t, None]]
        self.events = []
        self.status, self.end = "open", None
        self.iid, self.fold, self.uop, self.pipe = None, 1, 0, None

    def stage(self, name, t):
        last = self.stages[-1]
        if last[0] == name and last[2] is None:
            return
        if last[2] is None:
            last[2] = t
        self.stages.append([name, t, None])

    def finish(self, t, status):
        if self.stages[-1][2] is None:
            self.stages[-1][2] = t
        self.end, self.status = t, status


def replay(hist, t0, t1):
    """Apply the tracer rules at every rising edge; a = cycle ending at the edge, b = after it."""
    insns, retired = [], []
    id_q, ir_q, is_q = deque(), deque(), deque()
    groups = {}  # iid -> [Insn]: one ROB entry of one to three folded instructions
    checks = {"retire_slots": 0, "retired": 0, "retire_pc_match": 0, "retire_pc_mismatch": 0,
              "unknown_iid": 0, "fold_groups": 0}
    prev = None
    for t, b in samples(hist, range(t0 - t0 % 2, t1 + 1, 2)):
        a, prev = prev, b
        if a is None:
            continue
        # retire, oldest slot first
        for k in range(3):
            if not a[f"rt_vld{k}"]:
                continue
            checks["retire_slots"] += 1
            g = groups.pop(a[f"rt_iid{k}"], None)
            if g is None:
                checks["unknown_iid"] += 1  # dispatched before the replay started
                continue
            same = (g[0].pc >> 1) & 0x7FFF == a[f"rt_pc{k}"] & 0x7FFF
            checks["retire_pc_match" if same else "retire_pc_mismatch"] += 1
            checks["retired"] += len(g)
            for i in g:
                retired.append(i)
                i.stage("RT", t - 2)
                i.finish(t, "ok")
        # ROB flush: everything dispatched and not retired
        if a["rob_flush"]:
            for g in groups.values():
                for i in g:
                    i.finish(t, "aborted")
            groups.clear()
        # complete
        for n in CMPLT:
            if a[f"cm{n}"]:
                for i in groups.get(a[f"cm_iid{n}"], []):
                    if i.stages[-1][0] != "CM":
                        i.stage("CM", t)
                        break
        # LSU pipe registers (valid after the edge: in the stage during [t, t + 2))
        for name, s, _ in LSU_STAGES:
            if b[f"{s}_vld"]:
                for i in groups.get(b[f"{s}_iid"], []):
                    if i.stages[-1][0] != "CM":
                        i.stage(name, t)
                    break
        # RF, then execute unless the launch failed
        for n in PIPE_IID:
            if b[f"rf_vld{n}"]:
                for i in groups.get(b[f"rf_iid{n}"], []):
                    if i.stages[-1][0] == "IQ":
                        i.stage("RF", t)
                        i.pipe = n
                        break
            if a[f"rf_vld{n}"]:
                for i in groups.get(a[f"rf_iid{n}"], []):
                    if i.stages[-1][0] == "RF" and i.pipe == n:
                        if a[f"rf_fail{n}"]:
                            i.events.append(["replay", t])
                            i.stage("IQ", t)
                        elif n in PIPE_STAGE:
                            i.stage(PIPE_STAGE[n], t)
        # dispatch: ROB create port k takes the next inst_num instructions under one IID
        for k in range(4):
            if a[f"create_en{k}"]:
                num = max(1, bits(a[f"create_data{k}"], 18, 17))
                g = [is_q.popleft() for _ in range(min(num, len(is_q)))]
                for i in g:
                    i.iid, i.fold = a[f"create_iid{k}"], len(g)
                    i.stage("IQ", t)
                checks["fold_groups"] += len(g) > 1
                if g:
                    groups[a[f"create_iid{k}"]] = g
        # front-end flush: everything not dispatched
        if a["cancel"] or a["flush_fe"]:
            for q in (id_q, ir_q, is_q):
                for i in q:
                    i.finish(t, "aborted")
                q.clear()
            continue
        if not a["is_stall"]:
            for k in range(4):
                if a[f"ir_pd{k}"] and ir_q:
                    i = ir_q.popleft()
                    i.stage("IS", t)
                    is_q.append(i)
        if not a["ir_stall"]:
            last = None
            for k in range(4):
                if not a[f"id_pd{k}"]:
                    continue
                d = b[f"ir_data{k}"]
                pc, op = bits(d, 167, 153) << 1, bits(d, 31, 0)
                if id_q and id_q[0].pc == pc and (last is None or last.pc != pc):
                    i = id_q.popleft()
                    i.op = op
                    i.stage("IR", t)
                else:  # a further uop of a split instruction
                    i = Insn(pc, op, t, "IR")
                    i.uop = last.uop + 1 if last is not None and last.pc == pc else 1
                    insns.append(i)
                last = i
                ir_q.append(i)
        if not a["id_stall"]:
            for k in range(3):
                if b[f"id_vld{k}"]:
                    d = b[f"id_data{k}"]
                    i = Insn(bits(d, 63, 49) << 1, bits(d, 31, 0), t, "ID")
                    insns.append(i)
                    id_q.append(i)
    return insns, retired, checks


def disassembly(path):
    text = {}
    for line in open(path):
        m = re.match(r"\s*([0-9a-f]+):\s+[0-9a-f]+\s+(.*)$", line)
        if m:
            text[int(m.group(1), 16)] = re.sub(r"\s+", " ", m.group(2).split("#")[0].strip())
    return text


def program_order(retired, text):
    control = re.compile(r"^(b|j|c\.b|c\.j|ret|jal|jr|mret|ecall|ebreak|tail|call)")
    out = {"sequential": 0, "after_control_flow": 0, "broken": 0}
    for p, q in zip(retired, retired[1:]):
        size = 4 if p.op & 3 == 3 else 2
        if q.pc == p.pc + size or (q.pc == p.pc and q.uop > 1):
            out["sequential"] += 1
        elif control.match(text.get(p.pc, "")):
            out["after_control_flow"] += 1
        else:
            out["broken"] += 1
    return out


def demo_js(t0, t1, rows, checks, hist):
    """The compact literal embedded in docs/c910-verilator-tx-stream.html."""
    stages = ["ID", "IR", "IS", "IQ", "RF", "EX", "BJ", "AG", "DC", "DA", "WB", "CM", "RT"]
    cyc = lambda t: (t - t0) // 2
    texts, index, packed = [], {}, []
    for r in rows:
        index.setdefault(r["text"], len(index))
        if len(texts) < len(index):
            texts.append(r["text"])
        packed.append([r["seq"], r["pc"], index[r["text"]], ["ok", "aborted", "open"].index(r["status"]),
                       -1 if r["iid"] is None else r["iid"], r["fold"],
                       [[stages.index(s[0]), cyc(s[1]), cyc(s[2])] for s in r["stages"]]])
    names = [("iu_yy_xx_cancel", "cancel"), ("retire_rob_flush", "rob_flush"), ("retire_inst0_vld", "rt_vld0"),
             ("retire_inst1_vld", "rt_vld1"), ("retire_inst2_vld", "rt_vld2"), ("retire_inst0_iid", "rt_iid0")]
    waves = []
    for label, key in names:
        ch = [[0, next(v for t, v in reversed(hist[key]) if t <= t0)]]
        ch += [[cyc(t), v] for t, v in hist[key] if t0 < t <= t1 and t % 2 == 0]
        waves.append([label, ch])
    data = {"cycle0": t0 // 2, "cycles": cyc(t1), "stages": stages, "texts": texts, "rows": packed,
            "waves": waves, "checks": checks}
    return "const DATA = " + json.dumps(data, separators=(",", ":")) + ";\n"


def main():
    global VTR_FILE
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("vtr_file")
    ap.add_argument("disassembly", help="objdump -d of the program (coremark.dis)")
    ap.add_argument("--from", dest="t0", type=int, required=True, help="first time unit (even = rising edge)")
    ap.add_argument("--to", dest="t1", type=int, required=True)
    ap.add_argument("--vtr", default=str(ROOT / "target/release/vtr"))
    ap.add_argument("--json", help="write every instruction with its stages")
    ap.add_argument("--demo-js", help="write the documentation demo literal")
    args = ap.parse_args()
    VTR_FILE = args.vtr_file
    start = args.t0 - WARMUP
    hist = {k: history(args.vtr, p, start, args.t1) for k, p in SIG.items()}
    insns, retired, checks = replay(hist, start, args.t1)
    text = disassembly(args.disassembly)
    checks["program_order"] = program_order(retired, text)
    rows = []
    for i in insns:
        if i.begin < args.t0:
            continue
        if i.end is None:
            i.finish(args.t1, "open")
        rows.append({"seq": i.seq, "pc": i.pc, "insn": i.op, "text": text.get(i.pc, "?"), "begin": i.begin,
                     "end": i.end, "status": i.status, "iid": i.iid, "fold": i.fold, "uop": i.uop,
                     "pipe": i.pipe, "stages": i.stages, "events": i.events})
    status = {s: sum(r["status"] == s for r in rows) for s in ("ok", "aborted", "open")}
    print(json.dumps({"probes": len(SIG), "instructions": len(rows), "status": status, "checks": checks}))
    if args.json:
        pathlib.Path(args.json).write_text(json.dumps({"from": args.t0, "to": args.t1, "checks": checks, "insns": rows}))
    if args.demo_js:
        pathlib.Path(args.demo_js).write_text(demo_js(args.t0, args.t1, rows, checks, hist))
    ok = checks["retire_pc_mismatch"] == 0 and checks["program_order"]["broken"] == 0
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
