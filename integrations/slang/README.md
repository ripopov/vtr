# Standalone slang VDB exporter

The pinned pyslang adapter exports RTL design metadata independently of a
simulator build. The VDB schema, Rust library, debugging CLI and regression
fixtures live in `core/vtr-vdb`; simulator-integrated export remains in the
pinned Verilator fork.

| File | Responsibility |
|---|---|
| `export.py` | Elaborate RTL and write the separate VDB with source index |
| `requirements.txt` | Pinned pyslang version |
| `test_export.py` | Live elaboration, identity, source-index and committed-fixture checks |
| `verify_svg.py` | Parse and rasterize netlist SVG goldens for review |

Run from the repository root:

```sh
python3 -m venv .venv-vdb
.venv-vdb/bin/pip install -r integrations/slang/requirements.txt
.venv-vdb/bin/python integrations/slang/test_export.py
.venv-vdb/bin/python integrations/slang/export.py --top top -o design.vdb.json design.sv
```

See the [RTL schema and workflow](../../docs/VDB_RTL.md),
[VDB application note](../../docs/VDB_APPNOTE.md) and
[Verilator integration](../verilator/README.md).
