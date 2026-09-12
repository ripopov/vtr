# Volna: the primary debugging UI

Volna is the official VTR/VDB viewer. It currently displays VTR and FST
waveforms; VDB attachment, source views, transaction views and remote queries
remain planned work.

| Package | Responsibility |
|---|---|
| [volna-core](volna-core/) | Toolkit-independent document state, sessions, interaction and drawing |
| [volna](volna/README.md) | Main GPUI frontend for native, web and VS Code; web and extension assets live with this adapter |
| [volna-egui](volna-egui/README.md) | Minimal native frontend used to verify toolkit independence |

Run from the repository root with Rust 1.96+ and the relevant platform SDK:

```sh
cargo run -p volna --profile viewer -- volna/volna/examples/picorv32.vtr
cargo test -p volna-core
volna/volna/check.sh
volna/volna/web/build.sh
```

New viewer behavior belongs in `volna-core`; the GPUI frontend is the primary
feature target. Maintain egui's current functionality without requiring
feature parity. See the [architecture](volna/ARCHITECTURE.md),
[user/build guide](volna/README.md) and [verification guide](volna/VERIFICATION.md).
