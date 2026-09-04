# Verilator build of the pulp-c910 (openC910) SoC running CoreMark, with the
# whole design traced to FST or VTR.
#
#   make model MODE=none|fst|vtr     # Verilated model in $(BUILD)/obj_$(MODE)/Vtop
#   make sw                          # CoreMark image in $(BUILD)/sw/coremark
#
# MODE=vtr needs VTR_INCLUDE (directory of vtr.h) and VTR_LIBDIR (directory of
# libvtr.a); MODE=fst/vtr need a verilator with the matching --trace-* option
# ($(VERILATOR), default: the one on PATH). The model is single-threaded; host
# optimisation is -O2 for the fast path (the same for every mode).

HERE   := $(realpath $(CURDIR))
ROOT   := $(realpath $(CURDIR)/../../..)
BUILD  ?= $(HERE)/build
MODE   ?= none
JOBS   ?= $(shell nproc)
VERILATOR ?= verilator
ITERATIONS ?= 1

C910 := $(ROOT)/ext/pulp-c910
export CODE_BASE_PATH := $(C910)/vendor/thead_openc910/C910_RTL_FACTORY
export SMART_PATH     := $(C910)/vendor/thead_openc910/smart_run
export SYSMAP_INCDIR  := $(BUILD)/sysmap
export HERE

OBJDIR := $(BUILD)/obj_$(MODE)
MODEL  := $(OBJDIR)/Vtop

VFLAGS := --cc --exe --top-module top -Wno-fatal \
          -O3 --x-assign 0 --x-initial 0 --no-timing \
          --output-split 10000 --output-split-cfuncs 500 \
          --Mdir $(OBJDIR) -o Vtop -CFLAGS "-std=c++17"
ifeq ($(MODE),fst)
VFLAGS += --trace-fst
endif
ifeq ($(MODE),vtr)
VFLAGS += --trace-vtr
endif

.PHONY: model sw clean
model: $(MODEL)
sw: $(BUILD)/sw/coremark/inst.pat

# A sysmap.h that shadows the vendored one: identical, except that the lowest
# region, where the smart_run testbench keeps main memory, becomes cacheable.
# The vendored map targets the Cheshire SoC, where [0, 0x0100_0000) is the
# debug ROM and therefore non-cacheable; with the MMU off that would leave the
# whole smart_run memory uncached and CoreMark would take many times longer.
$(SYSMAP_INCDIR)/sysmap.h: $(CODE_BASE_PATH)/gen_rtl/mmu/rtl/sysmap.h
	@mkdir -p $(dir $@)
	sed -e "s/^\`define SYSMAP_FLG0 .*/\`define SYSMAP_FLG0        5'b01111/" \
	    -e 's/^`define \([A-Z0-9_]*\)/`undef \1\n`define \1/' $< > $@

# ct_lsu_ctrl.v declares two debug ports as [1:0] but their nets as
# [LFB_DATA_ENTRY-1:0]; Verilator rejects the conflict. The vendored file list is
# rewritten to use a copy with the port widths fixed (the nets' width).
export C910_FILELIST := $(BUILD)/C910_asic_rtl.fl
$(BUILD)/rtl/ct_lsu_ctrl.v: $(CODE_BASE_PATH)/gen_rtl/lsu/rtl/ct_lsu_ctrl.v
	@mkdir -p $(dir $@)
	sed -e 's/^input   \[1  :0\]  lsu_had_lfb_data_entry_\(last\|vld\);/input   [LFB_DATA_ENTRY-1:0]  lsu_had_lfb_data_entry_\1;/' $< > $@
$(C910_FILELIST): $(CODE_BASE_PATH)/gen_rtl/filelists/C910_asic_rtl.fl $(BUILD)/rtl/ct_lsu_ctrl.v
	sed -e 's|^.*/lsu/rtl/ct_lsu_ctrl.v$$|$(BUILD)/rtl/ct_lsu_ctrl.v|' $< > $@

$(MODEL): $(SYSMAP_INCDIR)/sysmap.h $(C910_FILELIST) rtl/c910_soc.f rtl/tc_sram.sv tb/tb_c910_bench.sv tb/sim_main.cpp
	@mkdir -p $(OBJDIR)
	$(VERILATOR) $(VFLAGS) -f rtl/c910_soc.f tb/tb_c910_bench.sv tb/sim_main.cpp > $(OBJDIR)/verilator.log 2>&1
	$(MAKE) -C $(OBJDIR) -f Vtop.mk -j$(JOBS) OPT_FAST="-O2" OPT_SLOW="-O1" \
	    VTR_INCLUDE="$(VTR_INCLUDE)" VTR_LIBDIR="$(VTR_LIBDIR)" > $(OBJDIR)/make.log 2>&1

$(BUILD)/sw/coremark/inst.pat:
	$(MAKE) -C sw BUILD=$(BUILD)/sw C910=$(C910) ITERATIONS=$(ITERATIONS)

clean:
	rm -rf $(BUILD)
