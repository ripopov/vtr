# Pipeline Studio: research and design decisions

Research reviewed September 5, 2026. This is a focused survey of established
visualization research, current interaction/accessibility guidance, and directly
comparable tools. “Best in class” is the design ambition, not a proven usability
ranking: this prototype has not undergone comparative studies with CPU designers.

## Konata presentation

Reviewed all 39 pages of Ryota Shioya's **Visualizing the out-of-order CPU model**,
linked by Konata as its ASPLOS 2018 gem5 tutorial. The currently linked PDF includes
later appendix updates (2022 note; PDF metadata dated January 2023).
[Presentation](https://github.com/shioyadan/Konata/wiki/gem5-konata.pdf).

- Pages 6–14: move from a limited text listing to spatial pan/zoom; retain PC and
  mnemonic beside the time view; overlay related runs.
- Pages 17–22: recognize out-of-order activity, wrong-path work, cache-miss bands,
  and throughput changes from the shape of execution.
- Pages 24–29: a surprising pattern is a starting point for investigation; aggregate
  counters alone do not explain causality.
- Pages 32–35: compare local timing changes, not just an overall speedup number.
- Pages 38–39: concurrent stage lanes, dependencies, custom stage information,
  and excluding flushed operations when reasoning about useful throughput.

Application: the examples include a long-latency load with two dependent consumers,
five squashed instructions after a branch, a steady trace, and an aligned baseline.
A small fixture rule preserves in-order retirement with up to two retirements per
cycle; completed younger operations have an explicit commit-wait stage.
The inspector gives explicit causes; IPC excludes squashed work. Distinct stage
names belong to this illustrative model, not to a claimed exact gem5 event mapping.

## Comparable projects

| Primary source | What it contributes | Applied here |
|---|---|---|
| [Konata current README](https://github.com/shioyadan/Konata/blob/master/README.md), also inspected in `ext/Konata` | Direct pan/zoom, fetch alignment, searchable instructions, command palette, saved locations, dependency controls, flushed visibility, and aligned overlays. Current browser version keeps processing local. | Fixed instruction labels, focus action, literal search, searchable commands, bookmarks, direct dependencies, explicit comparison alignment, local state. |
| [Perfetto UI guide](https://perfetto.dev/docs/visualization/perfetto-ui) | Event selection reveals details; F centers/fits selection; range selections, reversible filtering, keyboard commands, and pinned tracks support investigation. | Persistent inspector, focus selection, pointer and numeric range measurement, visible filter counts, pins, and direct commands. Filtered rows do not silently alter measurement scope. |
| [speedscope README](https://github.com/jlfwong/speedscope#views) | Separates chronological and aggregate questions; provides an overview and detailed view, direct navigation, frame focus, local processing, and offline distribution. | Persistent overview with viewport indicator; chronological instructions remain primary; summary metrics are secondary; no runtime dependencies or uploads. |
| [Ripes introduction](https://github.com/mortbopet/Ripes/blob/master/docs/introduction.md) | Pipeline stage table, cycle stepping, datapath selection, and visible execution state help learners connect program and processor. | Stage abbreviations, explicit cycle cursor, instruction stepping, and illustrative source context. A CPU circuit simulator would distract from this demo's trace-reading goal. |
| [gem5 visualization guide](https://www.gem5.org/documentation/general_docs/cpu_models/visualization/) | Correlates fetch/decode/rename/dispatch/issue/completion/retire events with PC, disassembly, and sequence identity. | Stable instruction IDs and PCs remain next to stage spans. No claim that the demo imports O3PipeView or reproduces a particular core. |
| [Chrome DevTools Performance reference](https://developer.chrome.com/docs/devtools/performance/reference) | Overview navigation, interval selection, selection details, and annotated performance investigation. | A stable overview and selected-range summary support moving from a pattern to an explanation without opening unrelated dialogs. |

## UI/UX research and accessibility

- [Nielsen's usability heuristics](https://www.nngroup.com/articles/ten-usability-heuristics/),
  reviewed January 2024: show current state, use domain vocabulary, make recovery
  obvious, favor recognition, support accelerators, and provide task-oriented help.
  Application: explicit selection, range, comparison labels, reset/clear actions,
  visible buttons paired with shortcuts, and task-based tutorial examples.
- [Navigating large information spaces](https://www.nngroup.com/articles/navigating-large-information-spaces/):
  context and landmarks reduce disorientation. Application: a persistent overview,
  saved views, event landmarks, and instruction pins.
- [W3C tabs pattern](https://www.w3.org/WAI/ARIA/apg/patterns/tabs/):
  associated tabs/panels, active state, roving tab stop, and arrow navigation.
  Applied to the two primary tabs; changing tabs retains the current investigation.
- [WCAG 2.2 minimum target guidance](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html):
  pointer targets need adequate size or spacing. Primary controls are 32–35px;
  compact instruction rows are 24px. Tiny zoomed-out stage cells have larger row
  selection and inspector alternatives; this is not a claim of full WCAG conformance.
- Labels and hatching supplement stage color; keyboard focus is visible; native
  dialogs constrain focus; Escape dismisses dialogs; theme and reduced-motion
  preferences are supported. All metrics are text, not color-only encodings.

## Product choices and tradeoffs

1. **One workflow, two tabs.** The viewer is a working instrument; the Field guide
   is a task-based tutorial with screenshots from that instrument. Secondary tools
   use a sidebar, inspector, or focused dialog rather than more top-level tabs.
2. **Direct manipulation plus precision.** Pan, zoom, overview navigation, and
   range dragging have explicit controls, numeric input, or keyboard alternatives.
3. **Selection is stable.** Filters can hide a selected row while the inspector
   preserves it and offers Focus to reveal it. Dependency navigation reveals a
   hidden producer automatically.
4. **State is explainable.** Baseline outlines and candidate fills share a cycle
   axis and match by instruction ID. No invented benchmark percentage. Range
   totals are computed from all example instructions, not the visible subset.
5. **Semantic detail follows scale.** Stage labels disappear at small widths;
   compact density removes secondary PCs. The inspector preserves exact values.
6. **Local investigation artifacts.** Pins, named view snapshots, per-trace notes,
   and theme persist locally when storage is available. Export downloads a JSON
   investigation snapshot. There is no backend, login, or faux file-import button.
7. **Honest scope.** Seventy-two synthetic instructions per trace, one core/thread,
   direct register dependencies, illustrative source context, deterministic
   comparison. No simulator engine, VTR parser, arbitrary trace import, critical-path
   inference, or cycle-accurate reproduction of a real processor is implied.

## Verification and next research

Browser tests exercise selection, dependency navigation, pan/zoom, range dragging
and input validation, filter/metric separation, comparison deltas, keyboard commands,
bookmarks, persistence, notes, download contents, all tutorial examples, screenshot
loading, and desktop/narrow layouts. Screenshots are captured from actual UI states,
not drawn approximations. See [README.md](README.md) for repeatable commands.

Before calling a production viewer best in class, conduct task-based sessions with
novice and expert CPU designers: locate a miss, explain a delayed consumer, isolate
wrong-path work, compare two builds, and restore an investigation. Measure success,
time, wrong turns, and confidence; test screen readers and alternative input devices.
Tune density and defaults from those results. This prototype makes those tasks
concrete enough to evaluate.
