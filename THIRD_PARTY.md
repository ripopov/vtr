# Third-party notices

The first-party license does not cover the following upstream material.

## C910 benchmark RTL

- `bench/workloads/c910/rtl/tc_sram.sv`: from
  [tech_cells_generic, src/rtl/tc_sram.sv](https://github.com/pulp-platform/tech_cells_generic/blob/master/src/rtl/tc_sram.sv),
  used by [pulp-c910](https://github.com/pulp-platform/pulp-c910)'s vendor patch 0009.
  Copyright (c) 2020 ETH Zurich and University of Bologna; author Wolfgang
  Roenninger. Licensed under [Solderpad Hardware License 0.51](LICENSES/SHL-0.51.txt).
  The local copy adds the provenance header; the behavioral SRAM is otherwise
  unchanged. Its existing copyright, attribution and license notices are retained.
- `bench/workloads/c910/tb/tb_c910_bench.sv`: derived from
  [openC910 smart_run/logical/tb/tb_verilator.v](https://github.com/T-head-Semi/openc910/blob/main/smart_run/logical/tb/tb_verilator.v),
  via `ext/pulp-c910/vendor/thead_openc910`.
  Copyright 2019–2021 T-Head Semiconductor Co., Ltd.
  Licensed under [Apache License 2.0](LICENSE-APACHE).
  Modifications remove `$dumpvars`, export cycle and retired-instruction counters
  and run status as ports, move termination to the C++ harness, and declare the
  core clock through `vtr_trace` in VTR builds. The file retains its attribution
  and prominent modification notice.

## Viewer assets and dependencies

See [Volna's font and icon notices](volna/volna/THIRD_PARTY.md) for Inter,
JetBrains Mono and Lucide, including links to their full licenses.
Desktop and VSIX distributions include these texts and a generated dependency
inventory with upstream license and notice files, including vendored native
libraries. Dependencies without separate license files include their upstream
source with the applicable license text so source attribution is retained.
System libraries supplied by the operating system are not bundled.

## Submodules

Submodules under `ext/` retain their own licenses and notices. The workspace
license does not relicense them; consult each upstream checkout before
redistributing its code or artifacts.
