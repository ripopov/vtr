# Builds the bare-metal CoreMark image for the openC910 smart_run SoC memory map.
# The CoreMark port and start-up code come from the vendored openC910 tree; the
# C910-specific CSR mnemonics and instructions used by crt0 are rewritten to plain
# RV64GC encodings so an upstream riscv64-unknown-elf toolchain can build them.

C910    ?= $(realpath $(CURDIR)/../../../..)/ext/pulp-c910
SMART     := $(C910)/vendor/thead_openc910/smart_run
LIB       := $(SMART)/tests/lib
CASES     := $(SMART)/tests/cases
SREC2VMEM := $(SMART)/tests/bin/Srec2vmem

BUILD   ?= build
CROSS   ?= riscv64-unknown-elf-
CC      := $(CROSS)gcc
OBJCOPY := $(CROSS)objcopy
OBJDUMP := $(CROSS)objdump

# CoreMark iterations. Start-up plus one iteration is ~240k cycles of RTL simulation.
ITERATIONS ?= 1

ARCH   := -march=rv64imafdc_zicsr_zifencei -mabi=lp64d -mcmodel=medany
COMMON := $(ARCH) -ffreestanding -fno-common -I$(CURDIR)/include \
          -Wno-implicit-function-declaration -Wno-builtin-declaration-mismatch
LDFLAGS = -T../link.ld -nostartfiles -nostdlib $(ARCH)

CM_CFLAGS := -O3 -funroll-all-loops -finline-limit=500 -fgcse-sm -fno-schedule-insns \
             -DPERFORMANCE_RUN=1 -include stdio.h

.PHONY: all clean
all: $(BUILD)/coremark/inst.pat

$(BUILD)/link.ld: $(LIB)/linker.lcf
	@mkdir -p $(dir $@)
	cp $< $@

# Rewrite the T-Head CSR mnemonics / custom `sync` opcode to portable forms, and
# drop two CSR writes that hang this SoC: MSMPR (0x7f3, TEE secure memory
# protection) and MCCR2 (0x7c3, L2 cache control - pulp-c910 has no internal L2).
$(BUILD)/crt0.S: $(LIB)/crt0.s
	@mkdir -p $(dir $@)
	sed -e 's/\bmxstatus\b/0x7c0/g'  -e 's/\bmhcr\b/0x7c1/g' \
	    -e 's/\bmcor\b/0x7c2/g'      -e 's/\bmccr2\b/0x7c3/g' \
	    -e 's/\bmhint\b/0x7c5/g'     -e 's/\bmsmpr\b/0x7f3/g' \
	    -e 's/^\( *\)sync *$$/\1fence/' \
	    -e '/csrsi 0x7f3/d' -e '/csrs *0x7c3/d' \
	    -e '/\.include "core_init.h"/d' $< > $@

$(BUILD)/coremark/inst.pat: $(BUILD)/crt0.S $(BUILD)/link.ld
	@rm -rf $(BUILD)/coremark && mkdir -p $(BUILD)/coremark
	@cp $(CASES)/coremark/* $(BUILD)/coremark/
	@rm -f $(BUILD)/coremark/cvt.c
	@# The vendor port hard-codes two iterations in core_main.c (its ITERATIONS define is unused).
	@sed -i 's/^\tresults\[0\].iterations= 2;$$/\tresults[0].iterations= $(ITERATIONS);/' $(BUILD)/coremark/core_main.c
	@grep -q 'iterations= $(ITERATIONS);' $(BUILD)/coremark/core_main.c
	@# RV64: the pointer-sized integer type must be 64 bit; the port ships a 32 bit one.
	@sed -i 's/^typedef ee_u32 ee_ptr_int;$$/typedef unsigned long ee_ptr_int;/' $(BUILD)/coremark/core_portme.h
	@cp $(LIB)/clib/fputc.c $(LIB)/newlib_wrap/printf.c src/libc_stub.c src/bench_support.c $(BUILD)/coremark/
	@cp $(LIB)/clib/config.h $(LIB)/clib/datatype.h $(LIB)/clib/uart.h $(LIB)/clib/timer.h $(BUILD)/coremark/
	@cp $(BUILD)/crt0.S $(BUILD)/coremark/
	cd $(BUILD)/coremark && $(CC) $(COMMON) $(CM_CFLAGS) -I. -c *.c
	cd $(BUILD)/coremark && $(CC) $(COMMON) -c crt0.S -o crt0.o
	cd $(BUILD)/coremark && $(CC) $(LDFLAGS) *.o -o coremark.elf -lgcc
	cd $(BUILD)/coremark && \
	  $(OBJCOPY) -O srec coremark.elf inst.hex -j '.text*' -j '.rodata*' -j '.eh_frame*' && \
	  $(OBJCOPY) -O srec coremark.elf data.hex -j '.data*' -j .bss -j .COMMON && \
	  $(SREC2VMEM) inst.hex inst.pat && \
	  $(SREC2VMEM) data.hex data.pat && \
	  $(OBJDUMP) -d coremark.elf > coremark.dis

clean:
	rm -rf $(BUILD)
