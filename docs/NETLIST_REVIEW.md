# Netlist SVG verification — 2026-09-05

The 25 SVG goldens in `crates/vtr-kdb/tests/goldens` were parsed as XML,
rasterized with `rsvg-convert` 2.62.3, and visually inspected in five review sheets.
The review covered every golden: text placement, value visibility, node
separation, port attachment, child-module boundaries, and missing-data styling.
The generated review gallery is `target/netlist-svg/review/index.html` and can
be recreated with `python3 tools/kdb/verify_svg.py`.

| Cases | Review result |
|---|---|
| `pipeline_initial`, `pipeline_reset`, `pipeline_first_edge`, `pipeline_hold`, `pipeline_late` | Two stage instances remain closed blue blocks. Mux inputs and interstage signals are visible; values change while geometry stays fixed. |
| `stage_reset`, `stage_first`, `stage_hold`, `stage_late`, `stage_second` | A single stage shows its clocked process and local ports. Recorded output values agree with the independent pipeline snapshots, including reset and enable hold. |
| `pipeline_missing`, `outside_range`, `dump_off`, `dump_resumed_stale` | Missing/invalid samples appear as red `unavailable`, with the reason in the tooltip. No zero substitution. |
| `dump_refreshed`, `semantics`, `semantics_changed`, `wrapper_prefix` | Four-state values and process pins remain readable. Bit selectors and concatenation are connected; refreshed samples return to normal value styling. |
| `hierarchical`, `nested_group`, `nested_leaf`, `generated_leaf` | Only immediate child instances appear; generate paths and parameter widths remain distinct. Replication is a compact operator. Long wide-bus values are abbreviated on screen and complete in tooltips. |
| `feedback_and_escaping`, `bidirectional_child` | Feedback routes outside the process block. Case-process inputs are retained. Net-declaration initializers, slice inputs, and the escaped name render. Parent inout connection reaches the `↔ io` pin; child view shows its tri-state mux connectivity. |
| `empty_module` | A readable empty-module message replaces an empty or malformed SVG. |

Automated Rust geometry checks run before comparing SVG goldens: finite positive
node rectangles, no block overlaps, one orthogonal route per wire, and exact
endpoint attachment to the intended ports. SVG verification independently checks
XML, route orthogonality after decimal rounding, unique node/pin identifiers,
and successful PNG rendering. Golden comparisons are exact, require explicit
`VTR_UPDATE_GOLDENS=1` to update, and also run through the CLI output path.

These are RTL connectivity snapshots. Process blocks preserve procedural
boundaries; the renderer is not a synthesizer or electrical simulator. Static
connectivity may include inactive branches. Numeric annotations come exclusively
from recorded signal samples, and unsupported evaluation retains diagnostics.
