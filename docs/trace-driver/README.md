# Trace Driver interface proposals for Surfer

## Share the presentation

Send **`index.html` alone**, or copy its complete contents into a UTF-8 file
named `trace-driver.html`. Open the saved file in a modern browser with
JavaScript enabled. No server, installation, external fonts, or network
connection is needed. Pasting HTML into an email body is not equivalent:
mail clients generally remove the JavaScript needed by the demos.

Russian is the default language. RU / EN switches the document language and
remembers the choice when browser storage is available. Demo labels stay in
English. Each demo has independent state and a **Reset demo** button.

The presentation compares four proposed interfaces, not a released Surfer
feature. Each “Try it” box describes working demo actions; the specification
blocks also discuss future capabilities. Large-tree loading, multiple queries,
workspace persistence, and Trace X are not implemented in these examples.

## Contents and data

The file includes a problem statement, a worked timing example, design
principles, four interactive HTML/SVG demos, a comparison table, and linked
vendor references. It embeds all source code, waveform histories, and trace
data needed to run the examples offline.

The common example comes from:

- `ext/surfer/examples/verilator/pipeline.{sv,vtr,vdb}`;
- `vtr-vdb trace pipeline.vdb pipeline.vtr top.q --time 26 --depth 14`
  (the original output is embedded in the HTML and retained separately in
  `trace-top.q-at-26.txt` for comparison);
- `vtr dump pipeline.vtr` for recorded value changes;
- `ext/surfer/themes/atlas-dark.toml` for the demo colors.

Times are picoseconds and displayed values are hexadecimal. The full tree has
27 nodes; folding port connections and hiding clock dependencies leaves 11
nodes when all branches are expanded. A node at `25⁻` refers to the state
immediately before 25 ps, while `25` refers to the settled state at that time.
Source annotations and waveform value columns follow the cursor; dependency
nodes retain their own sampling times.

## Working demo controls

| Demo | Controls |
| --- | --- |
| Driver Trace | Select and expand rows; toggle ports, clocks, controls and notes; filter names; limit display depth; add signals with `+`, double-click or Alt-click; use arrow keys, Enter to change the root, Backspace or Back to return, `a` to add, `c` / `f` to toggle, `/` to filter. |
| Temporal Flow | Select nodes or time-column headings; hover for details; toggle ports, clocks, controls and fit-to-width; scroll at 1:1; add a signal with double-click or Alt-click. |
| Source Code | Click an underlined identifier or a numbered cause; use number keys and Enter to follow dependencies; Alt+Left / Alt+Right and history entries navigate the trail. Signals are added automatically. |
| Waveform overlay | Select trace rows or arrows; hover arrows for both endpoint times; toggle ports, clocks, controls and arrows; collapse the group; clear the trace and restore it with Reset demo. |

Click a demo before using its keyboard shortcuts. A folded waveform row may
represent several trace nodes; clicking the row selects its first occurrence
in traversal order, while an arrow identifies a specific dependency. Demos
use fixed tile layouts, with scrolling inside the panels on small screens.

## Validation

The presentation was checked in headless Chrome with networking disabled,
including both languages, all four demos, keyboard navigation, filtering,
depth limits, clear/reset, language persistence, and layouts at 390, 768,
and 1440 pixels. The embedded source and values were compared with the
repository example and the CLI trace. These checks validate the fixed demos;
they do not establish performance on large designs.
