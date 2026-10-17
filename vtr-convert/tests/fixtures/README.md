# Conversion fixtures

These committed inputs exercise foreign-format conversion without a simulator,
submodule, or external checkout.

| Files | Coverage |
| --- | --- |
| `pipeline.kanata`, `waves.vcd` | Hand-authored Kanata and VCD conversion semantics |
| `values.fst`, `values-wrapped.fst` | Raw and gzip-wrapped FST: real values, arbitrary byte strings, nine-state logic, events, aliases, and a nonzero start time |
| `fst_types.fst` | FST declaration and value types, including ports |
| `features.fst`, `operators.fst`, `pipeline.fst` | Compared by Volna against paired VTR recordings in its `volna-trace/tests/fixtures` |
| `landing_dram.fst` | Source of the VTR landing DRAM example and scope-size fixture |
| `zlib.fst` | Compressed waveform histories and aliases |

## Provenance

The FST inputs and `fst_values.c` were imported from the
[prototype repository at revision 35d8aa8](https://github.com/ripopov/vtr/tree/35d8aa8fb1e328d95a0ceecd80fd0ed0bccc237c).
The `values` files and their generator came from `volna/volna-trace/tests/fixtures`;
the landing DRAM recording came from `volna/volna/examples`. That directory's
[guide](https://github.com/ripopov/vtr/blob/35d8aa8fb1e328d95a0ceecd80fd0ed0bccc237c/volna/volna/examples/README.md)
describes their original generators.

The FST type fixture and paired Verilator recordings came from the prototype's
Surfer fork, revision `cdcdfc85686b151953e1c6fa006ffe0591269754`, under
`examples/fst_types.fst` and `examples/verilator`. They are recorded regression
inputs; see [Verilator integration](../../../integrations/verilator/README.md) for simulator builds.

## Regeneration

`fst_values.c` uses GTKWave's FST writer API. With `fstapi.c`, `fstapi.h`, and
LZ4/zlib development libraries available:

```sh
cc vtr-convert/tests/fixtures/fst_values.c "$FST_WRITER_DIR/fstapi.c" \
  -I"$FST_WRITER_DIR" $(pkg-config --cflags --libs liblz4 zlib) \
  -o /tmp/volna-fst-values
/tmp/volna-fst-values vtr-convert/tests/fixtures/values.fst
/tmp/volna-fst-values vtr-convert/tests/fixtures/values-wrapped.fst wrap
```

Generate derived VTR fixtures using `vtr convert INPUT OUTPUT` with a new output
path, then replace the committed fixture. `values.vtr` and `landing_dram.vtr`
under `volna-trace/tests/fixtures`, and `volna/examples/landing_dram.vtr`, retain
the corresponding FST input's conversion semantics. Run conversion, loader,
server, and core tests after replacing them.
