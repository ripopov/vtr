// Pipeline tracer for the openC910 core (docs/c910-verilator-tx-stream.html).
//
// Bound to ct_core by c910_bind.sv; every port is a probe of an existing core
// signal, and nothing in the RTL changes. Each instruction becomes a
// transaction of TX.core0.pipeline (kind PIPELINE, generator "instruction"),
// counted in the core clock TX.core0.clk; the streams live in a root tree TX of
// their own, grouped per core (parameter GROUP), not deep in the instance tree.
//
// Identity: the tracer numbers instructions in program order as they enter IR
// (key space fe; split instructions give extra uops their own numbers there),
// gives each dispatched instruction a member name (mem = iid * 4 + position)
// and moves the oldest ones to their ROB entry's IID (iid) as a group of one to
// three folded instructions. Retire closes the group; a branch cancel or front-end
// flush aborts space fe, the ROB flush that follows aborts space iid, so IID
// reuse after a flush meets released keys only.
//
// Members: dispatch reports each instruction's destination physical register
// (PST slots, program order), which the tracer remembers as that register's
// producer. A pipe's register read names its instruction by destination
// register where the pipe has one (ALU and load pipes), otherwise it takes the
// entry's first waiting member without a destination; register read, execute,
// LSU stages and completion then belong to that member. At its first register
// read an instruction gets a wakeup relation from the in-flight producer of
// each source register.
//
// Timing: the block runs at the rising edge and sees pre-edge values. A pipeline
// register valid seen at edge T held during the cycle that began at the previous
// edge, so it enters its stage at t_prev (the RF, LSU, IR and ID steps below).
// Decisions taken on load conditions (retire, flushes, complete, launch,
// dispatch, IS) take effect now. IR and ID need the register contents after the
// edge, so the front end is applied one edge late with the conditions saved
// from the edge that loaded it. bench/workloads/c910/pipeline_replay.py runs the
// same rules over a recording of these probes as a differential check.
//
// Fetch: each instruction takes the IF, IP and IB times of the oldest fetch block
// that holds its PC (IB includes the wait in the instruction buffer), or LB when
// the loop buffer supplied it; then ID.
//
// Also recorded: split instructions' further uops (attribute uop and a uop_of
// relation to the instruction), launch replays and branch mispredictions as
// events, and dispatch stalls as a stage on the oldest instruction's stall lane.
// Two more trackers follow memory: store_queue (one store per store queue entry,
// parented to its store instruction: SQ until commit, CMT until it moves to the
// write merge buffer, WMB until it pops) and lsu_bus (the LSU's reads on the core
// bus by AXI id, parented to the load that missed: AR until the first beat, R).
//
// Shortcuts: multiply/divide complete at selection, and the store-data pipe,
// floating-point sources, fetch stages, bus writes (CoreMark's LSU issues none)
// and the write merge buffer's own traffic are not traced.
`timescale 1ns / 100ps

// Keys are the hardware's own identifiers, narrower than vtr_key_t.
// verilator lint_off WIDTHEXPAND
// verilator lint_off WIDTHTRUNC
// verilator tracing_off
module c910_vtr_tracer #(
    // Where the streams go: a scope of the file's own, one per core, beside the
    // design's instance tree. The core clock is declared there too (tb_c910_bench.sv).
    parameter string GROUP = "/TX.core0"
) (
    input logic               clk,
    input logic               cancel,  // branch mispredict: flush the front end
    input logic               flush_fe,  // RTU flushes the front end
    input logic               rob_flush,  // RTU flushes the ROB
    input logic               id_stall,
    ir_stall,
    is_stall,
    // Fetch: a block enters IF when IF is empty, cancelled or has just piped down; it
    // moves to IP at the pipedown and to the IB register when IP is valid and IB takes
    // it (not stalled, no pipe cancel). ID takes instructions from the instruction
    // buffer or IB (bypass), or from the loop buffer.
    input logic               if_pc_vld,
    if_cancel,
    if_pipedown,
    input logic               ip_to_ib,
    input logic [11:0]        ip_chunk,  // the IP block's PC[15:4]
    input logic               id_from_lbuf,
    input logic [2:0]         id_vld,  // ID register slots
    input logic [2:0][14:0]   id_pc,  // their PC[15:1] and opcode
    input logic [2:0][31:0]   id_op,
    input logic [3:0]         id_pd,  // ID -> IR per IR slot
    input logic [3:0][14:0]   ir_pc,  // IR register slots: PC[15:1] and opcode
    input logic [3:0][31:0]   ir_op,
    input logic [3:0]         ir_pd,  // IR -> IS
    input logic [3:0]         create_en,  // ROB create ports
    input logic [3:0][1:0]    create_num,  // instructions in the entry (0 = 1)
    input logic [3:0][6:0]    create_iid,
    input logic [3:0]         dis_dst_vld,  // dispatched instructions, program order: destination
    input logic [3:0][6:0]    dis_dst,  // physical register and IID
    input logic [3:0][6:0]    dis_dst_iid,
    input logic [7:0]         rf_vld,  // RF pipeline register per execution pipe
    input logic [7:0]         rf_fail,  // launch failure
    input logic [7:0][6:0]    rf_iid,
    input logic [7:0]         rf_dst_vld,  // its destination register (pipes 0, 1, 3)
    input logic [7:0][6:0]    rf_dst,
    input logic [7:0][2:0]    rf_src_vld,  // its source registers (pipes 0-4)
    input logic [7:0][2:0][6:0] rf_src,
    input logic [7:0]         cm,  // completion per pipe
    input logic [7:0][6:0]    cm_iid,
    input logic               bju_mispred,  // pipe 2 completes a mispredicted branch or jump
    input logic [6:0]         lsu_vld,  // ld AG DC DA WB, st AG DC DA
    input logic [6:0][6:0]    lsu_iid,
    input logic [2:0]         rt_vld,  // retire slots
    input logic [2:0][6:0]    rt_iid,
    input logic [2:0][38:0]   rt_pc,  // PC[39:1] of each retiring entry
    // Store queue entries: created by the store in st_da, committed, moved to the write
    // merge buffer, popped; flushed (uncommitted at a flush) or popped on an exception.
    input logic [11:0]        sq_create,
    input logic [11:0]        sq_cmit,
    input logic [11:0]        sq_to_wmb,
    input logic [11:0]        sq_pop,
    input logic [11:0]        sq_drop,
    input logic [11:0][6:0]   sq_iid,  // each entry's store
    // LSU read requests on the core bus: address handshake with id, address and source
    // (read buffer, write merge buffer, prefetcher), the read buffer entry and entry
    // IIDs, and read data beats.
    input logic               ar_hs,
    input logic [4:0]         ar_id,
    input logic [39:0]        ar_addr,
    input logic [2:0]         ar_src,  // pfu, wmb, rb
    input logic [7:0]         rb_ptr,
    input logic [7:0][6:0]    rb_iid,
    input logic               r_vld,
    input logic [4:0]         r_id,
    input logic               r_last,
    input logic               r_err
);
  import vtr_trace::*;
  // The disassembly hook (c910_label.cpp): "0x2c04 lh a6,0(t1)" for a PC.
  import "DPI-C" function string c910_vtr_label(int unsigned pc);

  localparam int ST_NONE = 0, ST_IQ = 1, ST_RF = 2, ST_EX = 3, ST_CM = 4;
  localparam bit [7:0] PIPES = 8'b1101_1111;  // pipes 0-4, 6, 7 exist
  localparam bit [7:0] EXEC = 8'b1100_0111;  // pipes with an execute stage of their own

  vtr_tracker_t p, stq, bus;
  vtr_keyspace_t fe, mem, iid, sq, rid;
  bit sq_open[12], sq_cmt[12];
  // Reads with one AXI id complete in order: the n-th read of id i is key i + 32 * n.
  longint unsigned ar_n[32], r_n[32];
  longint unsigned t_prev = 0;

  // Front end: instructions in ID not yet in IR (program order), and key counters.
  logic [15:0] q_pc[8];
  logic [31:0] q_op[8];
  longint unsigned q_t[8];
  int q_head = 0, q_n = 0;
  vtr_key_t n_ir = 0;  // last key given at IR
  vtr_key_t n_is = 0;  // last key moved to IS
  vtr_key_t n_disp = 0;  // last key dispatched
  vtr_key_t head_key = 0;  // the last instruction (not uop) that entered IR, and its PC
  logic [15:0] head_pc = 0;
  vtr_key_t stalled = 0;  // the instruction under a dispatch stall, 0 = none
  bit k_len4[64];  // per key (mod 64): a 4-byte instruction
  // Fetch blocks: the one in IF (since if_t), in IP (and when they entered IF and
  // IP), and those that reached IB, oldest first, with the last PC ID took from the
  // oldest.
  longint unsigned if_t = 0, ip_if_t = 0, ip_ip_t = 0;
  bit ip_full = 0;
  logic [11:0] b_chunk[16];
  longint unsigned b_if[16], b_ip[16], b_ib[16];
  int b_head = 0, b_n = 0;
  logic [15:0] b_last = 0;
  bit b_used = 0;
  // Fetch stages of what waits in ID: 0 none, 1 IF/IP/IB, 2 loop buffer.
  logic [1:0] q_src[8];
  longint unsigned q_if[8], q_ip[8], q_ib[8];
  // Conditions of the previous edge for the late front-end steps.
  bit p_flush = 1, p_ir_ok = 0, p_id_ok = 0, p_lbuf = 0;
  logic [3:0] p_id_pd = 0;
  // Per ROB entry: alive, members and their lengths.
  bit e_live[128];
  logic [1:0] e_n[128];
  bit e_len4[128][3];
  // Per member (iid * 4 + position): stage state, pipe, destination register, and
  // whether its sources were related already. Per physical register: its producer.
  logic [2:0] m_st[512];
  logic [2:0] m_pipe[512];
  bit m_dst[512];
  logic [6:0] m_reg[512];
  bit m_rel[512];
  logic [9:0] owner[128];  // producer member + 1, 0 = none in flight
  logic [8:0] rf_m[8];  // the member in each pipe's RF, found this edge
  bit rf_mv[8];

  function automatic string stage_of_pipe(int n);
    return n == 2 ? "BJ" : n >= 6 ? "FX" : "EX";
  endfunction

  initial begin
    p   = vtr_pipeline(GROUP, "pipeline", {GROUP, ".clk"});
    fe  = vtr_keyspace(p, "fe");
    mem = vtr_keyspace(p, "mem");
    iid = vtr_keyspace(p, "iid");
    stq = vtr_tracker(GROUP, "store_queue", "LSU", "store", {GROUP, ".clk"});
    sq  = vtr_keyspace(stq, "sq");
    bus = vtr_tracker(GROUP, "lsu_bus", "AXI", "read", {GROUP, ".clk"});
    rid = vtr_keyspace(bus, "rid");
    for (int k = 0; k < 12; k++) sq_open[k] = 0;
    for (int k = 0; k < 32; k++) begin
      ar_n[k] = 0;
      r_n[k]  = 0;
    end
    for (int e = 0; e < 128; e++) begin
      e_live[e] = 0;
      owner[e]  = 0;
    end
    for (int m = 0; m < 512; m++) m_st[m] = ST_NONE;
  end

  // Open key n_ir for the instruction at ID queue slot q: its fetch stages, then ID.
  task automatic open_from_id(int q);
    if (q_src[q] == 2) begin
      vtr_item_open_at(fe, n_ir, "LB", q_ib[q]);
      vtr_item_stage_at(fe, n_ir, "ID", q_t[q]);
    end else if (q_src[q] == 1) begin
      vtr_item_open_at(fe, n_ir, "IF", q_if[q]);
      vtr_item_stage_at(fe, n_ir, "IP", q_ip[q]);
      if (q_ib[q] < q_t[q]) vtr_item_stage_at(fe, n_ir, "IB", q_ib[q]);
      vtr_item_stage_at(fe, n_ir, "ID", q_t[q]);
    end else vtr_item_open_at(fe, n_ir, "ID", q_t[q]);
  endtask

  // An instruction or uop enters IR at t_prev: key n_ir, opened with the stages of ID
  // queue slot `q` (or first stage IR when it has no ID stage of its own, q < 0).
  task automatic enter_ir(logic [15:0] pc, logic [31:0] op, int q, int uop);
    n_ir++;
    k_len4[n_ir % 64] = op[1:0] == 2'b11;
    if (q >= 0) begin
      open_from_id(q);
      vtr_item_stage_at(fe, n_ir, "IR", t_prev);
    end else vtr_item_open_at(fe, n_ir, "IR", t_prev);
    vtr_item_attr_u64(fe, n_ir, "pc", pc);
    vtr_item_attr_u64(fe, n_ir, "insn", op);
    if (uop != 0) begin
      vtr_item_attr_u64(fe, n_ir, "uop", uop);
      if (head_pc == pc) vtr_item_relate("uop_of", fe, n_ir, fe, head_key);
    end else begin
      head_key = n_ir;
      head_pc  = pc;
    end
    vtr_item_label(fe, n_ir, c910_vtr_label(pc));
  endtask

  // The member of entry e in state st on pipe n (any pipe when n < 0), first by
  // position; -1 when none.
  function automatic int member_on(logic [6:0] e, int n, logic [2:0] st1, logic [2:0] st2);
    for (int j = 0; j < e_n[e]; j++)
      if ((m_st[e*4+j] == st1 || m_st[e*4+j] == st2) && (n < 0 || m_pipe[e*4+j] == n)) return e * 4 + j;
    return -1;
  endfunction

  // The member of entry e that ran on pipe n, in any state; -1 when none.
  function automatic int member_of_pipe(logic [6:0] e, int n);
    for (int j = 0; j < e_n[e]; j++) if (m_st[e*4+j] != ST_NONE && m_pipe[e*4+j] == n) return e * 4 + j;
    return -1;
  endfunction

  // The waiting member of entry e that pipe n reads registers for: the producer
  // of its destination register, else the first waiting member without one,
  // else the first waiting member.
  function automatic int member_for_rf(int n);
    logic [6:0] e;
    e = rf_iid[n];
    if (rf_dst_vld[n] && owner[rf_dst[n]] != 0 && (owner[rf_dst[n]] - 1) / 4 == e
        && m_st[owner[rf_dst[n]]-1] == ST_IQ)
      return owner[rf_dst[n]] - 1;
    for (int j = 0; j < e_n[e]; j++) if (m_st[e*4+j] == ST_IQ && !m_dst[e*4+j]) return e * 4 + j;
    return member_on(e, -1, ST_IQ, ST_IQ);
  endfunction

  // A member ends: its destination register has no producer in flight any more.
  task automatic release_member(int m);
    if (m_dst[m] && owner[m_reg[m]] == m + 1) owner[m_reg[m]] = 0;
    m_st[m] = ST_NONE;
  endtask

  always @(posedge clk) begin
    logic flush;
    int base;
    longint unsigned now;
    now = vtr_now();
    // ---- The previous cycle's pipeline registers (entered at t_prev) ----------------
    // LSU pipes: ld AG DC DA WB on pipe 3, st AG DC DA on pipe 4.
    for (int s = 0; s < 7; s++)
      if (lsu_vld[s] && e_live[lsu_iid[s]]) begin
        int m;
        m = member_on(lsu_iid[s], s < 4 ? 3 : 4, ST_RF, ST_EX);
        if (m >= 0) begin
          vtr_item_stage_at(mem, m, s == 0 || s == 4 ? "AG" : s == 1 || s == 5 ? "DC" : s == 3 ? "WB" : "DA", t_prev);
          m_st[m] = ST_EX;
        end
      end
    // RF: a waiting member was selected by pipe n; at its first register read, a
    // wakeup from the producer of each source register still in flight.
    for (int n = 0; n < 8; n++) begin
      rf_mv[n] = 0;
      if (PIPES[n] && rf_vld[n] && e_live[rf_iid[n]]) begin
        int m;
        m = member_for_rf(n);
        if (m >= 0) begin
          vtr_item_stage_at(mem, m, "RF", t_prev);
          m_st[m] = ST_RF;
          m_pipe[m] = n;
          rf_m[n] = m;
          rf_mv[n] = 1;
          if (!m_rel[m]) begin
            logic [9:0] from[3];  // one relation per producer, however many sources it feeds
            m_rel[m] = 1;
            for (int k = 0; k < 3; k++) begin
              from[k] = rf_src_vld[n][k] ? owner[rf_src[n][k]] : 0;
              if (from[k] != 0 && from[k] - 1 != m && (k < 1 || from[k] != from[0]) && (k < 2 || from[k] != from[1]))
                vtr_item_relate("wakeup", mem, from[k] - 1, mem, m);
            end
          end
        end
      end
    end
    // IR (loaded at t_prev unless the front end was flushed then): each IR slot is the
    // next instruction in ID, or a further uop of the previous slot's instruction.
    if (!p_flush && p_ir_ok) begin
      logic [15:0] last_pc;
      int last_uop;
      bit have_last;
      have_last = 0;
      last_uop = 0;
      last_pc = 0;
      for (int k = 0; k < 4; k++)
        if (p_id_pd[k]) begin
          logic [15:0] pc;
          logic [31:0] op;
          pc = {ir_pc[k], 1'b0};
          op = ir_op[k];
          if (q_n > 0 && q_pc[q_head] == pc && !(have_last && last_pc == pc)) begin
            enter_ir(pc, op, q_head, 0);
            q_head = (q_head + 1) % 8;
            q_n--;
            last_uop = 0;
          end else begin
            last_uop = have_last && last_pc == pc ? last_uop + 1 : 1;
            enter_ir(pc, op, -1, last_uop);
          end
          last_pc = pc;
          have_last = 1;
        end
    end
    // ID (loaded at t_prev unless stalled or flushed then), with the fetch stages of
    // the oldest fetch block holding each instruction's PC: blocks ID has moved past
    // (another 16-byte chunk, or a PC not after the last one taken) are dropped.
    if (!p_flush && p_id_ok)
      for (int k = 0; k < 3; k++)
        if (id_vld[k] && q_n < 8) begin
          int q;
          q = (q_head + q_n) % 8;
          q_pc[q] = {id_pc[k], 1'b0};
          q_op[q] = id_op[k];
          q_t[q]  = t_prev;
          q_src[q] = 0;
          if (p_lbuf) begin
            q_src[q] = 2;
            q_ib[q]  = t_prev - (now - t_prev);  // the cycle before ID
          end else begin
            while (b_n > 0 && (b_chunk[b_head] != id_pc[k][14:3] || b_used && {id_pc[k], 1'b0} <= b_last)) begin
              b_head = (b_head + 1) % 16;
              b_n--;
              b_used = 0;
            end
            if (b_n > 0) begin
              q_src[q] = 1;
              q_if[q] = b_if[b_head];
              q_ip[q] = b_ip[b_head];
              q_ib[q] = b_ib[b_head];
              b_last = {id_pc[k], 1'b0};
              b_used = 1;
            end
          end
          q_n++;
        end

    // ---- This edge, oldest first ----------------------------------------------------
    // Retire: each slot is one ROB entry of one to three instructions, which sat in the
    // retire stage during the previous cycle. Decode knew PC[15:1] only: where the full
    // PC has more bits, members get it and a new caption.
    for (int k = 0; k < 3; k++)
      if (rt_vld[k] && e_live[rt_iid[k]]) begin
        logic [39:0] pc;
        pc = {rt_pc[k], 1'b0};
        for (int j = 0; j < e_n[rt_iid[k]]; j++) begin
          if (pc[39:16] != 0) begin
            vtr_item_attr_u64(mem, rt_iid[k] * 4 + j, "pc", pc);
            vtr_item_label(mem, rt_iid[k] * 4 + j, c910_vtr_label(pc));
          end
          pc += e_len4[rt_iid[k]][j] ? 4 : 2;
          release_member(rt_iid[k] * 4 + j);
        end
        vtr_item_stage_at(iid, rt_iid[k], "RT", t_prev);
        vtr_item_close(iid, rt_iid[k], VTR_TX_OK);
        e_live[rt_iid[k]] = 0;
      end
    // ROB flush after a mispredicted branch retired: everything dispatched.
    if (rob_flush) begin
      void'(vtr_keyspace_abort(iid));
      for (int e = 0; e < 128; e++) begin
        e_live[e] = 0;
        owner[e]  = 0;
      end
      for (int m = 0; m < 512; m++) m_st[m] = ST_NONE;
    end
    // Completion, by the member that launched on the pipe (else the entry's first one
    // not complete); the branch unit reports its mispredictions with it.
    for (int n = 0; n < 8; n++)
      if (PIPES[n] && cm[n] && e_live[cm_iid[n]]) begin
        int m;
        m = member_on(cm_iid[n], n, ST_RF, ST_EX);
        if (m < 0) m = member_on(cm_iid[n], -1, ST_IQ, ST_RF);
        if (m < 0) m = member_on(cm_iid[n], -1, ST_EX, ST_EX);
        if (m >= 0) begin
          if (n == 2 && bju_mispred) vtr_item_event(mem, m, "mispredict");
          vtr_item_stage(mem, m, "CM");
          m_st[m] = ST_CM;
        end
      end
    // Launch from RF: execute, or a replay back to IQ.
    for (int n = 0; n < 8; n++)
      if (rf_mv[n] && rf_vld[n] && m_st[rf_m[n]] == ST_RF && m_pipe[rf_m[n]] == n) begin
        if (rf_fail[n]) begin
          vtr_item_event(mem, rf_m[n], "replay");
          vtr_item_stage(mem, rf_m[n], "IQ");
          m_st[rf_m[n]] = ST_IQ;
        end else if (EXEC[n]) begin
          vtr_item_stage(mem, rf_m[n], stage_of_pipe(n));
          m_st[rf_m[n]] = ST_EX;
        end
      end
    // Store queue: pops and drops first, then commits and moves, then new entries,
    // whose parent is the store that creates them from st_dc.
    for (int k = 0; k < 12; k++)
      if (sq_open[k] && (sq_pop[k] || sq_drop[k])) begin
        vtr_item_close(sq, k, sq_pop[k] ? VTR_TX_OK : VTR_TX_ABORTED);
        sq_open[k] = 0;
      end
    for (int k = 0; k < 12; k++) begin
      if (sq_open[k] && sq_cmit[k] && !sq_cmt[k]) begin
        vtr_item_stage(sq, k, "CMT");
        sq_cmt[k] = 1;
      end
      if (sq_open[k] && sq_to_wmb[k]) vtr_item_stage(sq, k, "WMB");
    end
    for (int k = 0; k < 12; k++)
      if (sq_create[k]) begin
        int m;
        vtr_item_open(sq, k, "SQ");
        vtr_item_attr_u64(sq, k, "entry", k);
        sq_open[k] = 1;
        sq_cmt[k]  = 0;
        m = e_live[lsu_iid[5]] ? member_on(lsu_iid[5], 4, ST_RF, ST_EX) : -1;
        if (m >= 0) vtr_item_parent(sq, k, mem, m);
      end
    // Core bus reads: a beat belongs to the oldest open read of its id, and the last
    // beat ends it. An address handshake begins one. A read of the read buffer belongs
    // to the load or store that owns the issuing entry, or, once that store retired,
    // to its store queue entry.
    if (r_vld && r_n[r_id] < ar_n[r_id]) begin
      vtr_item_stage(rid, r_id + 32 * r_n[r_id], "R");
      if (r_last) begin
        vtr_item_close(rid, r_id + 32 * r_n[r_id], r_err ? VTR_TX_ERROR : VTR_TX_OK);
        r_n[r_id]++;
      end
    end
    if (ar_hs) begin
      vtr_key_t key;
      key = ar_id + 32 * ar_n[ar_id];
      ar_n[ar_id]++;
      vtr_item_open(rid, key, "AR");
      vtr_item_attr_u64(rid, key, "addr", ar_addr);
      vtr_item_attr_u64(rid, key, "id", ar_id);
      vtr_item_attr_str(rid, key, "source", ar_src[0] ? "rb" : ar_src[1] ? "wmb" : "pfu");
      if (ar_src[0])
        for (int k = 0; k < 8; k++)
          if (rb_ptr[k]) begin
            int m;
            m = e_live[rb_iid[k]] ? member_of_pipe(rb_iid[k], 3) : -1;
            if (m < 0 && e_live[rb_iid[k]]) m = member_of_pipe(rb_iid[k], 4);
            if (m >= 0) vtr_item_parent(rid, key, mem, m);
            else
              for (int q = 0; q < 12; q++)
                if (sq_open[q] && sq_iid[q] == rb_iid[k]) begin
                  vtr_item_parent(rid, key, sq, q);
                  break;
                end
          end
    end
    // Dispatch: create port k names the oldest instructions in IS by one IID; the
    // dispatched instructions report their destination registers in program order.
    // On a ROB flush edge the ROB resets its create pointer instead: those
    // instructions leave IS and are flushed with the rest.
    base = 0;
    for (int k = 0; k < 4; k++)
      if (create_en[k]) begin
        int num;
        num = create_num[k] == 0 ? 1 : create_num[k];
        if (num > n_is - n_disp) num = n_is - n_disp;
        if (rob_flush) begin
          if (num > 0) void'(vtr_item_bind_oldest(fe, num, iid, create_iid[k]));
          n_disp += num;
          continue;
        end
        for (int j = 0; j < num; j++) begin
          int m, s;
          m = create_iid[k] * 4 + j;
          s = base + j;
          vtr_item_bind(fe, n_disp + 1 + j, mem, m);
          e_len4[create_iid[k]][j] = k_len4[(n_disp+1+j)%64];
          m_st[m] = ST_IQ;
          m_pipe[m] = 0;
          m_rel[m] = 0;
          m_dst[m] = s < 4 && dis_dst_vld[s] && dis_dst_iid[s] == create_iid[k];
          m_reg[m] = dis_dst[s];
          if (m_dst[m]) owner[dis_dst[s]] = m + 1;
        end
        base += num;
        if (num > 0) begin
          void'(vtr_item_bind_oldest(fe, num, iid, create_iid[k]));
          vtr_item_stage(iid, create_iid[k], "IQ");
          vtr_item_attr_u64(iid, create_iid[k], "iid", create_iid[k]);
          vtr_item_attr_u64(iid, create_iid[k], "fold", num);
          e_live[create_iid[k]] = 1;
          e_n[create_iid[k]] = num;
          n_disp += num;
        end
      end
    if (rob_flush) void'(vtr_keyspace_abort(iid));
    // Front-end flush: everything not dispatched, including what waits in ID.
    flush = cancel || flush_fe;
    if (flush) begin
      for (int i = 0; i < q_n; i++) begin
        n_ir++;
        open_from_id((q_head + i) % 8);
        vtr_item_attr_u64(fe, n_ir, "pc", q_pc[(q_head+i)%8]);
        vtr_item_attr_u64(fe, n_ir, "insn", q_op[(q_head+i)%8]);
        vtr_item_label(fe, n_ir, c910_vtr_label(q_pc[(q_head+i)%8]));
      end
      void'(vtr_keyspace_abort(fe));
      q_n = 0;
      n_is = n_ir;
      n_disp = n_ir;
    end else if (!is_stall) begin
      // IS: the oldest instructions in IR move on.
      for (int k = 0; k < 4; k++)
        if (ir_pd[k] && n_is < n_ir) begin
          n_is++;
          vtr_item_stage(fe, n_is, "IS");
        end
    end
    // A dispatch stall holds the oldest instruction in IS: a stage on its stall lane.
    if (stalled != 0 && (!is_stall || flush || stalled <= n_disp)) begin
      if (!flush && stalled > n_disp) vtr_item_lane(fe, stalled, "stall", "");
      stalled = 0;
    end
    if (is_stall && !flush && stalled == 0 && n_is > n_disp) begin
      stalled = n_disp + 1;
      vtr_item_lane(fe, stalled, "stall", "dispatch");
    end
    // Fetch blocks move: IP moves on to IB, IF pipes down to IP, and a new block enters
    // IF. A front-end flush empties everything.
    if (flush) begin
      b_n = 0;
      b_used = 0;
      ip_full = 0;
    end else begin
      if (ip_to_ib && ip_full) begin
        if (b_n == 16) begin
          b_head = (b_head + 1) % 16;
          b_n--;
          b_used = 0;
        end
        b_chunk[(b_head+b_n)%16] = ip_chunk;
        b_if[(b_head+b_n)%16] = ip_if_t;
        b_ip[(b_head+b_n)%16] = ip_ip_t;
        b_ib[(b_head+b_n)%16] = now;
        b_n++;
        ip_full = 0;
      end
      if (if_pipedown) begin
        ip_if_t = if_t;
        ip_ip_t = now;
        ip_full = 1;
      end
    end
    if (flush || if_pipedown || if_cancel || !if_pc_vld) if_t = now;
    p_flush = flush;
    p_ir_ok = !ir_stall;
    p_id_ok = !id_stall;
    p_id_pd = id_pd;
    p_lbuf  = id_from_lbuf;
    t_prev  = now;
  end
endmodule
// verilator tracing_on
// verilator lint_on WIDTHTRUNC
// verilator lint_on WIDTHEXPAND
