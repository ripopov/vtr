# Surfer trace-driver UI/UX studies

`index.html` is a self-contained bilingual presentation (Russian by default,
RU / EN switch at the top of the navigation; the choice is remembered in the
browser). Open it in any browser; it only fetches Google Fonts and works
without them. It contains:

1. the problem statement;
2. design principles for a tiling viewer;
3. four live mockups of the proposed UI, all driven by the same example data:
   - `ext/surfer/examples/verilator/pipeline.{sv,vtr,vdb}`
   - the dependency tree from `vtr-vdb trace pipeline.vdb pipeline.vtr top.q --time 26 --depth 14`
     (verbatim copy in `trace-top.q-at-26.txt`)
   - value changes from `vtr dump pipeline.vtr`
   - colors from `ext/surfer/themes/atlas-dark.toml`;
4. a comparison matrix;
5. a brief overview of driver tracing in Verdi, SimVision/Indago,
   Questa/Visualizer, Aldec, DVE and the static IDEs.

Mockups are plain HTML/SVG/JS with no dependencies; click nodes, rows, arrows,
tokens and toggles. Variant 3 uses keyboard keys after the mockup is clicked.
The mockup chrome stays in English in both languages because it depicts the
proposed UI and the design's signal names. The document describes a target
UI/UX and deliberately does not depend on the current state of the code.
