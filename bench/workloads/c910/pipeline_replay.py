#!/usr/bin/env python3
"""Differential checker for the C910 pipeline tracer (docs/c910-verilator-tx-stream.html).

tb/c910_tracer.sv records every C910 instruction as a VTR transaction while the
simulation runs. This script runs the same rules after the fact, from the
recorded probe signals of a capture that also dumped the whole design
(`capture.sh --pipeline`), over a model of the keyed tracker (vtr_track.hpp),
and compares the result with the recorded pipeline stream: every transaction's
times, status, stages and attributes must be identical. It then checks the
reconstruction against the core's own retire signals: the decode PC of each
retiring entry's first instruction against the retire PC, and program order of
the retired stream against the program's disassembly, and that the VDB profile
c910.pipeline.vdb.json describes every stage name the tracer recorded.

It reads only the 94 probe signals, through `vtr value` and `vtr changes`, and
walks the rising clock edges (even time units in this harness) from the start of
the run, because the tracer's state starts there. At edge T it sees the values
just before the edge (the dump at T - 1), as the tracer does.

    pipeline_replay.py <capture.vtr> <coremark.dis> [--to T] [--no-compare]
        [--json out.json] [--demo-js out.js --from T0 --to T1] [--vtr path/to/vtr]

--no-compare replays a recording made without the tracer. --demo-js writes the
literal embedded in the proposal page for the window [--from, --to].
"""
import argparse
import collections
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
CORE = "TOP.top.x_soc.x_cpu_sub_system_axi.x_rv_integration_platform.x_cpu_top.x_ct_top_0.x_ct_core."
GROUP = "TX.core0."  # the tracer's streams: a root tree of their own, per core
STREAM = GROUP + "pipeline"
IDU = "x_ct_idu_top."
LSU = "x_ct_lsu_top."
RT = "x_ct_rtu_top.x_ct_rtu_rob.x_ct_rtu_rob_rt."

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
PIPE_STAGE = {0: "EX", 1: "EX", 2: "BJ", 6: "FX", 7: "FX"}  # pipes with an execute stage of their own
for n, path in PIPE_IID.items():
    sig(f"rf_vld{n}", IDU + f"x_ct_idu_rf_ctrl.rf_pipe{n}_inst_vld")
    sig(f"rf_fail{n}", IDU + f"x_ct_idu_rf_ctrl.ctrl_rf_pipe{n}_lch_fail")
    sig(f"rf_iid{n}", path, 7)
CMPLT = {0: "iu_rtu_pipe0", 1: "iu_rtu_pipe1", 2: "iu_rtu_pipe2", 3: "lsu_rtu_wb_pipe3",
         4: "lsu_rtu_wb_pipe4", 6: "vfpu_rtu_pipe6", 7: "vfpu_rtu_pipe7"}
for n, base in CMPLT.items():
    sig(f"cm{n}", base + "_cmplt")
    sig(f"cm_iid{n}", base + "_iid", 7)
sig("bht_mispred", "iu_rtu_pipe2_bht_mispred")
for k in range(4):
    sig(f"dis_dst_vld{k}", f"idu_rtu_pst_dis_inst{k}_preg_vld")
    sig(f"dis_dst{k}", f"idu_rtu_pst_dis_inst{k}_preg", 7)
    sig(f"dis_dst_iid{k}", f"idu_rtu_pst_dis_inst{k}_preg_iid", 7)
IFU = "x_ct_ifu_top."
sig("if_pc_vld", IFU + "x_ct_ifu_ifctrl.if_pc_vld")
sig("if_cancel", IFU + "x_ct_ifu_ifctrl.if_cancel")
sig("if_pipedown", IFU + "x_ct_ifu_ifctrl.ifctrl_ifdp_pipedown")
sig("ip_vld", IFU + "x_ct_ifu_ipctrl.ip_vld")
sig("ib_stall", IFU + "x_ct_ifu_ipctrl.ibctrl_ipctrl_stall")
sig("ip_cancel", IFU + "x_ct_ifu_ipctrl.pcgen_ipctrl_pipe_cancel")
sig("ip_vpc", IFU + "x_ct_ifu_ipdp.ipdp_bht_vpc", 39)
sig("id_from_lbuf", IFU + "x_ct_ifu_ibctrl.ibctrl_ibdp_lbuf_inst_vld")
SQ = LSU + "x_ct_lsu_sq."
sig("sq_create", SQ + "sq_entry_create_vld", 12)
for k in range(12):
    e = SQ + f"x_ct_lsu_sq_entry_{k}."
    sig(f"sq_cmit{k}", e + "sq_entry_cmit_set")
    sig(f"sq_to_wmb{k}", e + "sq_entry_pop_to_ce_grnt_x")
    sig(f"sq_pop{k}", e + "sq_entry_pop_vld")
    sig(f"sq_flush{k}", e + "sq_entry_flush_pop_vld")
    sig(f"sq_expt{k}", e + "sq_entry_expt_pop_vld")
    sig(f"sq_async{k}", e + "rtu_lsu_async_flush")
    sig(f"sq_iid{k}", SQ + f"sq_entry_iid_{k}", 7)
sig("ar_req", LSU + "lsu_biu_ar_req")
sig("ar_ready", LSU + "biu_lsu_ar_ready")
sig("ar_id", LSU + "lsu_biu_ar_id", 5)
sig("ar_addr", LSU + "lsu_biu_ar_addr", 40)
for src in ("rb", "wmb", "pfu"):
    sig(f"ar_{src}", LSU + f"x_ct_lsu_bus_arb.bus_arb_{src}_ar_sel")
sig("rb_ptr", LSU + "x_ct_lsu_rb.rb_biu_req_ptr", 8)
for k in range(8):
    sig(f"rb_iid{k}", LSU + f"x_ct_lsu_rb.rb_entry_iid_{k}", 7)
sig("r_vld", LSU + "biu_lsu_r_vld")
sig("r_id", LSU + "biu_lsu_r_id", 5)
sig("r_last", LSU + "biu_lsu_r_last")
sig("r_resp", LSU + "biu_lsu_r_resp", 4)
STREAMS = {GROUP + "pipeline": "pipeline", GROUP + "store_queue": "store_queue", GROUP + "lsu_bus": "lsu_bus"}
SPACE_STREAM = {"fe": "pipeline", "mem": "pipeline", "iid": "pipeline", "sq": "store_queue", "rid": "lsu_bus"}
RF_DATA_WIDTH = {0: 227, 1: 214, 2: 82, 3: 163, 4: 163}
for n, width in RF_DATA_WIDTH.items():
    sig(f"rf_data{n}", IDU + f"x_ct_idu_rf_dp.rf_pipe{n}_data", width)


def rf_dst(n, v):
    """The destination register in pipe n's RF register (ALU and load pipes), or None.
    Fields of ct_idu_rf_dp.v: AIQ0/AIQ1 and LSIQ DST_VLD 42, DST_PREG 50:44."""
    if n not in (0, 1, 3):
        return None
    d = v[f"rf_data{n}"]
    return bits(d, 50, 44) if bits(d, 42, 42) else None


def rf_srcs(n, v):
    """The source registers in pipe n's RF register: AIQ SRC0/1/2 (valid 39-41,
    pregs 66:60, 76:70, 86:80), BIQ SRC0/1 (39-40; 49:43, 59:53), LSIQ SRC0/1 (39-40;
    66:60, 76:70). Floating-point pipes have none."""
    if n not in RF_DATA_WIDTH:
        return []
    d = v[f"rf_data{n}"]
    fields = {0: [(39, 66), (40, 76), (41, 86)], 1: [(39, 66), (40, 76), (41, 86)], 2: [(39, 49), (40, 59)],
              3: [(39, 66), (40, 76)], 4: [(39, 66), (40, 76)]}[n]
    return [bits(d, hi, hi - 6) if bits(d, vld, vld) else None for vld, hi in fields]
sig("jmp_mispred", "iu_rtu_pipe2_jmp_mispred")
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


def history(vtr, file, path, t1):
    """[(time, value)]: every change of a probe up to t1, from its value at time 0."""
    name = CORE + path
    first = subprocess.run([vtr, "value", file, name, "0"], capture_output=True, text=True)
    if first.returncode != 0:
        sys.exit(f"{name}: {first.stderr.strip()}")
    out = [(0, bin_value(first.stdout.split()[-1]))]
    changes = subprocess.run([vtr, "changes", file, name, "--from", "0", "--to", str(t1)],
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
        yield row


def bits(v, hi, lo):
    return (v >> lo) & ((1 << (hi - lo + 1)) - 1)


# ---- A model of vtr::Tracker (vtr_track.hpp) and of the writer's stage rules ------------

class Item:
    def __init__(self, seq, begin, stream):
        self.seq, self.begin, self.stream = seq, begin, stream
        self.parent = None
        self.stages = []  # [name, lane, begin, end or None]
        self.attrs = {}
        self.events = []
        self.names = {}  # space -> key
        self.relations = []  # (kind, Item) from this item
        self.lanes = {}  # lane -> open stage name
        self.status, self.end = None, None

    def enter(self, name, lane, t):
        if self.lanes.get(lane) == name:
            return  # already there: the stage continues
        for s in reversed(self.stages):
            if s[1] == lane and s[3] is None:
                s[3] = max(t, s[2])
                break
        self.stages.append([name, lane, t, None])
        self.lanes[lane] = name


class Tracker:
    def __init__(self):
        self.seq = 0
        self.items = {}  # seq -> Item, open items
        self.spaces = {}  # space -> {key: [Item] in open order}
        self.ended = []
        self.diags = {}

    def diag(self, what):
        self.diags[what] = self.diags.get(what, 0) + 1

    def group(self, sp, k, what):
        g = self.spaces.setdefault(sp, {}).get(k)
        if not g:
            self.diag(f"{what} on a key that names no item")
        return list(g or [])

    def name(self, it, sp, k):
        if it.names.get(sp) == k:
            return
        if sp in it.names:
            self.unname(it, sp)
        it.names[sp] = k
        g = self.spaces.setdefault(sp, {}).setdefault(k, [])
        g.append(it)
        g.sort(key=lambda i: i.seq)

    def unname(self, it, sp):
        k = it.names.pop(sp)
        g = self.spaces[sp][k]
        g.remove(it)
        if not g:
            del self.spaces[sp][k]

    def evict(self, sp, k, keep, t, what):
        stale = [i for i in self.spaces.setdefault(sp, {}).get(k, []) if i not in keep]
        if stale:
            self.diag(what)
            for i in stale:
                self.end(i, "aborted", t)

    def open(self, sp, k, stage, t):
        self.evict(sp, k, [], t, "open on a key that still names an item; that item is aborted")
        self.seq += 1
        it = Item(self.seq, t, SPACE_STREAM[sp])
        self.items[it.seq] = it
        it.enter(stage, "", t)
        self.name(it, sp, k)

    def bind(self, sp, k, to, tk, t):
        g = self.group(sp, k, "bind")
        if not g:
            return
        self.evict(to, tk, g, t, "bind to a key that still names another item; that item is aborted")
        for it in g:
            self.name(it, to, tk)

    def bind_oldest(self, sp, n, to, tk, t):
        moved = sorted((i for g in self.spaces.setdefault(sp, {}).values() for i in g), key=lambda i: i.seq)[:n]
        if len(moved) < n:
            self.diag("bind_oldest found fewer items than requested")
        self.evict(to, tk, moved, t, "bind to a key that still names another item; that item is aborted")
        for it in moved:
            self.unname(it, sp)
            self.name(it, to, tk)
        return len(moved)

    def stage(self, sp, k, name, t):
        for it in self.group(sp, k, "stage"):
            it.enter(name, "", t)

    def event(self, sp, k, name, t):
        for it in self.group(sp, k, "event"):
            it.events.append((name, t))

    def lane(self, sp, k, lane, name, t):
        for it in self.group(sp, k, "lane"):
            if name:
                it.enter(name, lane, t)
            elif it.lanes.get(lane):
                for s in reversed(it.stages):
                    if s[1] == lane and s[0] == it.lanes[lane] and s[3] is None:
                        s[3] = max(t, s[2])
                        break
                it.lanes[lane] = None

    def parent(self, sp, k, bsp, bk):
        a, b = self.group(sp, k, "parent"), self.group(bsp, bk, "parent")
        if b:
            for x in a:
                x.parent = b[0]

    def relate(self, kind, sp, k, bsp, bk):
        a, b = self.group(sp, k, "relate"), self.group(bsp, bk, "relate")
        for x in a:
            for y in b:
                x.relations.append((kind, y))

    def attr(self, sp, k, key, v):
        for it in self.group(sp, k, "attr"):
            it.attrs[key] = v

    def close(self, sp, k, status, t):
        for it in self.group(sp, k, "close"):
            self.end(it, status, t)

    def abort_space(self, sp, t):
        g = sorted((i for g in self.spaces.setdefault(sp, {}).values() for i in g), key=lambda i: i.seq)
        for it in g:
            self.end(it, "aborted", t)

    def end(self, it, status, t):
        it.end = max(t, it.begin)
        for s in it.stages:
            if s[3] is None:
                s[3] = max(it.end, s[2])
        it.status = status
        for sp in list(it.names):
            self.unname(it, sp)
        del self.items[it.seq]
        self.ended.append(it)

    def all_items(self, t_close):
        """Every item in open order; items still open end with status open at t_close."""
        out = list(self.ended)
        for it in list(self.items.values()):
            it.end = max(t_close, it.begin)
            for s in it.stages:
                if s[3] is None:
                    s[3] = max(it.end, s[2])
            it.status = "open"
            out.append(it)
        return sorted(out, key=lambda i: i.seq)


# ---- The rules of tb/c910_tracer.sv -------------------------------------------------------

def replay(hist, t_last, labels):
    """Run the tracer at every rising edge 2 <= T <= t_last; `v` holds the values just before T."""
    tr = Tracker()
    ST_NONE, ST_IQ, ST_RF, ST_EX, ST_CM = range(5)
    e_live, e_n, e_len4 = [False] * 128, [0] * 128, [[False] * 3 for _ in range(128)]
    m_st, m_pipe, m_dst, m_reg, m_rel = [ST_NONE] * 512, [0] * 512, [False] * 512, [0] * 512, [False] * 512
    owner = [0] * 128  # producer member + 1 per physical register
    rf_m, rf_mv = [0] * 8, [False] * 8
    sq_open, sq_cmt = [False] * 12, [False] * 12
    ar_n, r_n = [0] * 32, [0] * 32  # reads of one AXI id complete in order: key id + 32 * n
    queue = []  # ID: [pc, op, t, src, t_if, t_ip, t_ib]
    # Fetch blocks: IF since if_t; the IP one; those that reached IB [chunk, if, ip, ib].
    if_t, ip_blk, blocks, b_last, b_used = 0, None, [], 0, False
    p_lbuf = False
    n_ir = n_is = n_disp = 0
    head_key, head_pc, stalled = 0, 0, 0
    k_len4 = {}
    p_flush, p_ir_ok, p_id_ok, p_id_pd = True, False, False, [0] * 4
    t_prev = 0
    checks = {"retire_slots": 0, "retired": 0, "retire_pc_match": 0, "retire_pc_mismatch": 0, "fold_groups": 0,
              "replays": 0, "mispredicts": 0, "dispatch_stalls": 0, "wakeups": 0, "rf_by_register": 0,
              "rf_by_position": 0, "stores": 0, "stores_parented": 0, "bus_reads": 0, "bus_reads_parented": 0}
    order = []  # retired items in retire order

    def open_from_id(q):
        pc, op, t_id, src, t_if, t_ip, t_ib = q
        if src == 2:
            tr.open("fe", n_ir, "LB", t_ib)
            tr.stage("fe", n_ir, "ID", t_id)
        elif src == 1:
            tr.open("fe", n_ir, "IF", t_if)
            tr.stage("fe", n_ir, "IP", t_ip)
            if t_ib < t_id:
                tr.stage("fe", n_ir, "IB", t_ib)
            tr.stage("fe", n_ir, "ID", t_id)
        else:
            tr.open("fe", n_ir, "ID", t_id)

    def enter_ir(pc, op, q, uop):
        nonlocal n_ir, head_key, head_pc
        n_ir += 1
        k_len4[n_ir % 64] = op & 3 == 3
        if q is not None:
            open_from_id(q)
            tr.stage("fe", n_ir, "IR", t_prev)
        else:
            tr.open("fe", n_ir, "IR", t_prev)
        tr.attr("fe", n_ir, "pc", pc)
        tr.attr("fe", n_ir, "insn", op)
        if uop:
            tr.attr("fe", n_ir, "uop", uop)
            if head_pc == pc:
                tr.relate("uop_of", "fe", n_ir, "fe", head_key)
        else:
            head_key, head_pc = n_ir, pc
        tr.attr("fe", n_ir, "vtr.label", labels(pc))

    def member_on(e, n, st1, st2):
        for j in range(e_n[e]):
            m = e * 4 + j
            if m_st[m] in (st1, st2) and (n < 0 or m_pipe[m] == n):
                return m
        return -1

    def member_for_rf(n, v):
        e = v[f"rf_iid{n}"]
        d = rf_dst(n, v)
        if d is not None and owner[d] and (owner[d] - 1) // 4 == e and m_st[owner[d] - 1] == ST_IQ:
            checks["rf_by_register"] += 1
            return owner[d] - 1
        checks["rf_by_position"] += 1
        for j in range(e_n[e]):
            if m_st[e * 4 + j] == ST_IQ and not m_dst[e * 4 + j]:
                return e * 4 + j
        return member_on(e, -1, ST_IQ, ST_IQ)

    def member_of_pipe(e, n):
        for j in range(e_n[e]):
            m = e * 4 + j
            if m_st[m] != ST_NONE and m_pipe[m] == n:
                return m
        return -1

    def release_member(m):
        if m_dst[m] and owner[m_reg[m]] == m + 1:
            owner[m_reg[m]] = 0
        m_st[m] = ST_NONE

    edges = range(2, t_last + 1, 2)
    for T, v in zip(edges, samples(hist, (T - 1 for T in edges))):
        # The previous cycle's pipeline registers.
        for s, (name, key, _) in enumerate(LSU_STAGES):
            e = v[f"{key}_iid"]
            if v[f"{key}_vld"] and e_live[e]:
                m = member_on(e, 3 if s < 4 else 4, ST_RF, ST_EX)
                if m >= 0:
                    tr.stage("mem", m, name, t_prev)
                    m_st[m] = ST_EX
        for n in range(8):
            rf_mv[n] = False
            if n in PIPE_IID and v[f"rf_vld{n}"] and e_live[v[f"rf_iid{n}"]]:
                m = member_for_rf(n, v)
                if m >= 0:
                    tr.stage("mem", m, "RF", t_prev)
                    m_st[m], m_pipe[m] = ST_RF, n
                    rf_m[n], rf_mv[n] = m, True
                    if not m_rel[m]:
                        m_rel[m] = True
                        froms = []  # one relation per producer, however many sources it feeds
                        for src in rf_srcs(n, v):
                            o = owner[src] if src is not None else 0
                            if o and o - 1 != m and o not in froms:
                                checks["wakeups"] += 1
                                tr.relate("wakeup", "mem", o - 1, "mem", m)
                            froms.append(o)
        if not p_flush and p_ir_ok:
            last_pc, last_uop = None, 0
            for k in range(4):
                if not p_id_pd[k]:
                    continue
                d = v[f"ir_data{k}"]
                pc, op = bits(d, 167, 153) << 1, bits(d, 31, 0)
                if queue and queue[0][0] == pc and last_pc != pc:
                    q = queue.pop(0)
                    enter_ir(pc, op, q, 0)
                    last_uop = 0
                else:
                    last_uop = last_uop + 1 if last_pc == pc else 1
                    enter_ir(pc, op, None, last_uop)
                last_pc = pc
        if not p_flush and p_id_ok:
            for k in range(3):
                if v[f"id_vld{k}"] and len(queue) < 8:
                    d = v[f"id_data{k}"]
                    pc = bits(d, 63, 49) << 1
                    q = [pc, bits(d, 31, 0), t_prev, 0, 0, 0, 0]
                    if p_lbuf:
                        q[3], q[6] = 2, t_prev - (T - t_prev)
                    else:
                        while blocks and (blocks[0][0] != pc >> 4 & 0xFFF or b_used and pc <= b_last):
                            blocks.pop(0)
                            b_used = False
                        if blocks:
                            q[3:7] = [1] + blocks[0][1:4]
                            b_last, b_used = pc, True
                    queue.append(q)
        # This edge, oldest first.
        for k in range(3):
            e = v[f"rt_iid{k}"]
            if not v[f"rt_vld{k}"] or not e_live[e]:
                continue
            checks["retire_slots"] += 1
            members = sorted(tr.spaces["iid"][e], key=lambda i: i.seq)
            checks["retire_pc_match" if members[0].attrs["pc"] >> 1 & 0x7FFF == v[f"rt_pc{k}"] & 0x7FFF
                   else "retire_pc_mismatch"] += 1
            pc = v[f"rt_pc{k}"] << 1
            for j in range(e_n[e]):
                if pc >> 16:
                    tr.attr("mem", e * 4 + j, "pc", pc)
                    tr.attr("mem", e * 4 + j, "vtr.label", labels(pc))
                pc += 4 if e_len4[e][j] else 2
                release_member(e * 4 + j)
            order += members
            checks["retired"] += len(members)
            tr.stage("iid", e, "RT", t_prev)
            tr.close("iid", e, "ok", T)
            e_live[e] = False
        if v["rob_flush"]:
            tr.abort_space("iid", T)
            e_live = [False] * 128
            owner = [0] * 128
            m_st = [ST_NONE] * 512
        for n in CMPLT:
            e = v[f"cm_iid{n}"]
            if v[f"cm{n}"] and e_live[e]:
                m = member_on(e, n, ST_RF, ST_EX)
                if m < 0:
                    m = member_on(e, -1, ST_IQ, ST_RF)
                if m < 0:
                    m = member_on(e, -1, ST_EX, ST_EX)
                if m >= 0:
                    if n == 2 and (v["bht_mispred"] or v["jmp_mispred"]):
                        checks["mispredicts"] += 1
                        tr.event("mem", m, "mispredict", T)
                    tr.stage("mem", m, "CM", T)
                    m_st[m] = ST_CM
        for n in range(8):
            m = rf_m[n]
            if rf_mv[n] and v[f"rf_vld{n}"] and m_st[m] == ST_RF and m_pipe[m] == n:
                if v[f"rf_fail{n}"]:
                    checks["replays"] += 1
                    tr.event("mem", m, "replay", T)
                    tr.stage("mem", m, "IQ", T)
                    m_st[m] = ST_IQ
                elif n in PIPE_STAGE:
                    tr.stage("mem", m, PIPE_STAGE[n], T)
                    m_st[m] = ST_EX
        # Store queue: pops and drops, commits and moves, new entries.
        for k in range(12):
            pop = v[f"sq_pop{k}"]
            if sq_open[k] and (pop or v[f"sq_flush{k}"] or v[f"sq_expt{k}"] or v[f"sq_async{k}"]):
                tr.close("sq", k, "ok" if pop else "aborted", T)
                sq_open[k] = False
        for k in range(12):
            if sq_open[k] and v[f"sq_cmit{k}"] and not sq_cmt[k]:
                tr.stage("sq", k, "CMT", T)
                sq_cmt[k] = True
            if sq_open[k] and v[f"sq_to_wmb{k}"]:
                tr.stage("sq", k, "WMB", T)
        for k in range(12):
            if v["sq_create"] >> k & 1:
                tr.open("sq", k, "SQ", T)
                tr.attr("sq", k, "entry", k)
                sq_open[k], sq_cmt[k] = True, False
                checks["stores"] += 1
                e = v["st_dc_iid"]
                m = member_on(e, 4, ST_RF, ST_EX) if e_live[e] else -1
                if m >= 0:
                    checks["stores_parented"] += 1
                    tr.parent("sq", k, "mem", m)
        # Core bus reads.
        rid = v["r_id"]
        if v["r_vld"] and r_n[rid] < ar_n[rid]:
            tr.stage("rid", rid + 32 * r_n[rid], "R", T)
            if v["r_last"]:
                tr.close("rid", rid + 32 * r_n[rid], "error" if v["r_resp"] & 2 else "ok", T)
                r_n[rid] += 1
        if v["ar_req"] and v["ar_ready"]:
            aid = v["ar_id"]
            key = aid + 32 * ar_n[aid]
            ar_n[aid] += 1
            tr.open("rid", key, "AR", T)
            tr.attr("rid", key, "addr", v["ar_addr"])
            tr.attr("rid", key, "id", aid)
            tr.attr("rid", key, "source", "rb" if v["ar_rb"] else "wmb" if v["ar_wmb"] else "pfu")
            checks["bus_reads"] += 1
            if v["ar_rb"]:
                for k in range(8):
                    if not v["rb_ptr"] >> k & 1:
                        continue
                    e = v[f"rb_iid{k}"]
                    m = member_of_pipe(e, 3) if e_live[e] else -1
                    if m < 0 and e_live[e]:
                        m = member_of_pipe(e, 4)
                    if m >= 0:
                        checks["bus_reads_parented"] += 1
                        tr.parent("rid", key, "mem", m)
                    else:
                        for q in range(12):
                            if sq_open[q] and v[f"sq_iid{q}"] == e:
                                checks["bus_reads_parented"] += 1
                                tr.parent("rid", key, "sq", q)
                                break
        base = 0
        for k in range(4):
            if not v[f"create_en{k}"]:
                continue
            e = v[f"create_iid{k}"]
            num = min(bits(v[f"create_data{k}"], 18, 17) or 1, n_is - n_disp)
            if v["rob_flush"]:  # the ROB resets its create pointer instead
                if num > 0:
                    tr.bind_oldest("fe", num, "iid", e, T)
                n_disp += num
                continue
            for j in range(num):
                m, sl = e * 4 + j, base + j
                tr.bind("fe", n_disp + 1 + j, "mem", m, T)
                e_len4[e][j] = k_len4[(n_disp + 1 + j) % 64]
                m_st[m], m_pipe[m], m_rel[m] = ST_IQ, 0, False
                m_dst[m] = sl < 4 and bool(v[f"dis_dst_vld{sl}"]) and v[f"dis_dst_iid{sl}"] == e
                m_reg[m] = v[f"dis_dst{sl}"] if sl < 4 else 0
                if m_dst[m]:
                    owner[m_reg[m]] = m + 1
            base += num
            if num > 0:
                tr.bind_oldest("fe", num, "iid", e, T)
                tr.stage("iid", e, "IQ", T)
                tr.attr("iid", e, "iid", e)
                tr.attr("iid", e, "fold", num)
                checks["fold_groups"] += num > 1
                e_live[e], e_n[e] = True, num
                n_disp += num
        if v["rob_flush"]:
            tr.abort_space("iid", T)
        flush = bool(v["cancel"] or v["flush_fe"])
        if flush:
            for q in queue:
                pc, op = q[0], q[1]
                n_ir += 1
                open_from_id(q)
                tr.attr("fe", n_ir, "pc", pc)
                tr.attr("fe", n_ir, "insn", op)
                tr.attr("fe", n_ir, "vtr.label", labels(pc))
            tr.abort_space("fe", T)
            queue = []
            n_is = n_disp = n_ir
        elif not v["is_stall"]:
            for k in range(4):
                if v[f"ir_pd{k}"] and n_is < n_ir:
                    n_is += 1
                    tr.stage("fe", n_is, "IS", T)
        if stalled and (not v["is_stall"] or flush or stalled <= n_disp):
            if not flush and stalled > n_disp:
                tr.lane("fe", stalled, "stall", "", T)
            stalled = 0
        if v["is_stall"] and not flush and not stalled and n_is > n_disp:
            stalled = n_disp + 1
            checks["dispatch_stalls"] += 1
            tr.lane("fe", stalled, "stall", "dispatch", T)
        if flush:
            blocks, b_used, ip_blk = [], False, None
        else:
            if v["ip_vld"] and not v["ib_stall"] and not v["ip_cancel"] and ip_blk is not None:
                if len(blocks) == 16:
                    blocks.pop(0)
                    b_used = False
                blocks.append([v["ip_vpc"] >> 3 & 0xFFF, ip_blk[0], ip_blk[1], T])
                ip_blk = None
            if v["if_pipedown"]:
                ip_blk = (if_t, T)
        if flush or v["if_pipedown"] or v["if_cancel"] or not v["if_pc_vld"]:
            if_t = T
        p_flush, p_ir_ok, p_id_ok = flush, not v["ir_stall"], not v["id_stall"]
        p_id_pd = [v[f"id_pd{k}"] for k in range(4)]
        p_lbuf = bool(v["id_from_lbuf"])
        t_prev = T
    return tr, order, checks


def disassembly(path):
    text = {}
    for line in open(path):
        m = re.match(r"\s*([0-9a-f]+):\s+[0-9a-f]+\s+(.*)$", line)
        if m:
            text[int(m.group(1), 16)] = re.sub(r"\s+", " ", m.group(2).split("#")[0].strip())
    return text


def program_order(retired, text):
    control = re.compile(r"^(b|j|c\.b|c\.j|ret|jal|jr|mret|ecall|ebreak|tail|call)")
    out = {"sequential": 0, "after_control_flow": 0, "outside_program": 0, "broken": 0}
    for p, q in zip(retired, retired[1:]):
        if p.attrs["pc"] not in text or q.attrs["pc"] not in text:
            out["outside_program"] += 1
            continue
        size = 4 if p.attrs["insn"] & 3 == 3 else 2
        if q.attrs["pc"] == p.attrs["pc"] + size or (q.attrs["pc"] == p.attrs["pc"] and q.attrs.get("uop", 0) > 0):
            out["sequential"] += 1
        elif control.match(text.get(p.attrs["pc"], "")):
            out["after_control_flow"] += 1
        else:
            out["broken"] += 1
    return out


def recorded(vtr, file, t_last):
    """The tracer's transactions (all three streams) from `vtr tx`, in open (id) order, up to t_last."""
    out, cur = [], None
    proc = subprocess.Popen([vtr, "tx", file, "--from", "0", "--to", str(t_last), "--max", "100000000"],
                            stdout=subprocess.PIPE, text=True)
    for line in proc.stdout:
        if line.startswith("tx "):
            m = re.match(r"tx (\d+) (\S+)/\S+ \[(\d+) \.\. (\d+)\] status=(\w+) kind=\w+(?: parent=(\d+))?", line)
            cur = None
            if m[2] in STREAMS:
                cur = {"id": int(m[1]), "stream": STREAMS[m[2]], "begin": int(m[3]), "end": int(m[4]), "status": m[5],
                       "parent": int(m[6]) if m[6] else None, "attrs": {}, "stages": [], "events": []}
                out.append(cur)
        elif cur is None:
            continue
        elif m := re.match(r"    stage (\S+) lane=(\S*) \[(\d+) \.\. (\d+)\]", line):
            cur["stages"].append([m[1], m[2], int(m[3]), int(m[4])])
        elif m := re.match(r"    event @(\d+) (\S+)", line):
            cur["events"].append((m[2], int(m[1])))
        elif m := re.match(r"    (\S+) = (.*)$", line):
            val = m[2]
            cur["attrs"][m[1]] = json.loads(val) if val.startswith('"') else int(val)
    if proc.wait():
        sys.exit("vtr tx failed")
    return sorted(out, key=lambda t: t["id"])


def recorded_relations(vtr, file, ids):
    """Counter of (from id, kind, to id) leaving the given transactions, from `vtr tx --id`."""
    out = collections.Counter()
    ids = sorted(ids)
    for i in range(0, len(ids), 1000):
        text = subprocess.check_output([vtr, "tx", file, "--id", ",".join(map(str, ids[i:i + 1000]))], text=True)
        current = None
        for line in text.splitlines():
            if m := re.match(r"tx (\d+) ", line):
                current = int(m[1])
            elif m := re.match(r"    -> (\S+) (\d+)", line):
                out[(current, m[1], int(m[2]))] += 1
    return out


def relation_count(vtr, file):
    info = subprocess.check_output([vtr, "info", file], text=True)
    return int(re.search(r"tx blocks:\s*\d+ \(\d+ transactions, (\d+) relations\)", info)[1])


def compare(items, live, t_last):
    """Items the replay ended by t_last against the recorded ones, matched in open order."""
    mismatches = []
    compared = 0
    tx_of = {it.seq: rec["id"] for it, rec in zip(items, live)}
    for it, rec in zip(items, live):
        if it.status == "open":
            continue  # still running at the end of the replayed range
        compared += 1
        want = {"stream": it.stream, "begin": it.begin, "end": it.end, "status": it.status, "attrs": it.attrs,
                "stages": it.stages, "events": it.events,
                "parent": tx_of.get(it.parent.seq) if it.parent else None}
        got = {k: rec[k] for k in want}
        if want != got:
            mismatches.append((rec["id"], want, got))
    return compared, mismatches


def demo_js(t0, t1, rows, checks, hist, clock):
    """The compact literal embedded in docs/c910-verilator-tx-stream.html: the rows
    (transaction dicts) that begin in [t0, t1], their main-lane stages in cycles
    from t0, and the recorded flush and retire probes."""
    stages = ["IF", "IP", "IB", "LB", "ID", "IR", "IS", "IQ", "RF", "EX", "BJ", "FX", "AG", "DC", "DA", "WB", "CM", "RT"]
    first, period = clock
    cyc = lambda t: (t - t0) // period
    texts, index, packed = [], {}, []
    for r in rows:
        if r["begin"] < t0 or r["begin"] > t1:
            continue
        text = r["attrs"]["vtr.label"].split(" ", 1)[1]
        if text not in index:
            index[text] = len(texts)
            texts.append(text)
        status = r["status"] if r["end"] <= t1 else "open"
        packed.append([r["id"], r["attrs"]["pc"], index[text], ["ok", "aborted", "open"].index(status),
                       r["attrs"].get("iid", -1), r["attrs"].get("fold", 1),
                       [[stages.index(s[0]), cyc(s[2]), cyc(min(s[3], t1))] for s in r["stages"]
                        if s[1] == "" and s[2] <= t1]])
    names = [("iu_yy_xx_cancel", "cancel"), ("retire_rob_flush", "rob_flush"), ("retire_inst0_vld", "rt_vld0"),
             ("retire_inst1_vld", "rt_vld1"), ("retire_inst2_vld", "rt_vld2"), ("retire_inst0_iid", "rt_iid0")]
    waves = []
    for label, key in names:
        ch = [[0, next(v for t, v in reversed(hist[key]) if t <= t0)]]
        ch += [[cyc(t), v] for t, v in hist[key] if t0 < t <= t1 and (t - first) % period == 0]
        waves.append([label, ch])
    # Cycles are the declared clock's (vtr clocks), as Volna counts them.
    data = {"cycle0": (t0 - first) // period, "cycles": cyc(t1), "stages": stages, "texts": texts, "rows": packed,
            "waves": waves, "checks": checks}
    return "const DATA = " + json.dumps(data, separators=(",", ":")) + ";\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("vtr_file")
    ap.add_argument("disassembly", help="objdump -d of the program (coremark.dis)")
    ap.add_argument("--to", dest="t1", type=int, help="last time unit replayed (default: the end of the run)")
    ap.add_argument("--from", dest="t0", type=int, default=0, help="first time unit of the --demo-js window")
    ap.add_argument("--no-compare", action="store_true", help="the recording has no pipeline stream")
    ap.add_argument("--vtr", default=str(ROOT / "target/release/vtr"))
    ap.add_argument("--json", help="write every replayed instruction with its stages")
    ap.add_argument("--demo-js", help="write the documentation demo literal for [--from, --to]")
    args = ap.parse_args()
    info = subprocess.check_output([args.vtr, "info", args.vtr_file], text=True)
    end_of_run = int(re.search(r"time range:\s*\d+ \.\. (\d+)", info)[1])
    if args.t1 is None:
        args.t1 = end_of_run
    text = disassembly(args.disassembly)
    labels = lambda pc: f"0x{pc:04x} {text.get(pc, '?')}"
    hist = {k: history(args.vtr, args.vtr_file, p, args.t1) for k, p in SIG.items()}
    tr, order, checks = replay(hist, args.t1, labels)
    items = tr.all_items(args.t1)
    checks["program_order"] = program_order(order, text)
    profile = json.loads((pathlib.Path(__file__).parent / "c910.pipeline.vdb.json").read_text())
    recorded_stages = sorted({s[0] for i in items if i.stream == "pipeline" for s in i.stages})
    checks["stages"] = recorded_stages
    checks["stages_without_profile"] = [s for s in recorded_stages if s not in profile["stages"]]
    checks["tracker_diagnostics"] = tr.diags
    status = {s: sum(i.status == s for i in items if i.stream == "pipeline") for s in ("ok", "aborted", "open")}
    checks["streams"] = {st: sum(i.stream == st for i in items) for st in ("pipeline", "store_queue", "lsu_bus")}
    report = {"probes": len(SIG), "edges": args.t1 // 2, "instructions": checks["streams"]["pipeline"], "status": status,
              "checks": checks}
    ok = (checks["retire_pc_mismatch"] == 0 and checks["program_order"]["broken"] == 0
          and not checks["stages_without_profile"])
    if not args.no_compare:
        live = recorded(args.vtr, args.vtr_file, args.t1)
        compared, mismatches = compare(items, live, args.t1)
        # Relations: every replayed one is in the file, and over a whole run no other.
        tx_of = {it.seq: rec["id"] for it, rec in zip(items, live)}
        want = collections.Counter((tx_of[it.seq], kind, tx_of[to.seq]) for it in items for kind, to in it.relations
                                   if it.seq in tx_of and to.seq in tx_of)
        got = recorded_relations(args.vtr, args.vtr_file, {f for f, _, _ in want})
        missing = want - got
        whole = args.t1 >= end_of_run
        extra = relation_count(args.vtr, args.vtr_file) - sum(want.values()) if whole else 0
        report["differential"] = {"recorded": len(live), "compared": compared, "mismatches": len(mismatches),
                                  "relations": sum(want.values()), "relations_missing": sum(missing.values()),
                                  "relations_extra": extra}
        mismatches += [(f, "relation", (k, t)) for f, k, t in sorted(missing)[:5]]
        for tx_id, want, got in mismatches[:5]:
            print(f"tx {tx_id}:\n  replay   {want}\n  recorded {got}", file=sys.stderr)
        ok = ok and not mismatches and compared > 0 and extra == 0
    print(json.dumps(report))
    if args.json:
        pathlib.Path(args.json).write_text(json.dumps([
            {"seq": i.seq, "stream": i.stream, "begin": i.begin, "end": i.end, "status": i.status, "attrs": i.attrs,
             "stages": i.stages, "events": i.events} for i in items]))
    if args.demo_js:
        # The recorded stream when there is one (identical to the replay, as checked above).
        rows = [r for r in live if r["stream"] == "pipeline"] if not args.no_compare else [
            {"id": i.seq, "begin": i.begin, "end": i.end, "status": i.status, "attrs": i.attrs, "stages": i.stages}
            for i in items if i.stream == "pipeline"]
        out = subprocess.check_output([args.vtr, "clocks", args.vtr_file], text=True).splitlines()
        m = re.match(r"\s+\[(\d+) \.\. \d+\] period (\d+)", out[1])
        pathlib.Path(args.demo_js).write_text(demo_js(args.t0, args.t1, rows, checks, hist, (int(m[1]), int(m[2]))))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
