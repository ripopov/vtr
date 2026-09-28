# Volna website UI kit

Kit sources use the `.kit` extension (JSX loaded by Babel via `<script type="text/babel" src>`) so the design-system compiler does not bundle them.

Click-through recreation of the Volna marketing + docs site, composed only from the design-system components (window.VolnaDesignSystem_3096af).

- index.html — shell; hash routes #/, #/benchmarks, #/vtr, #/docs, #/download. Theme switch (light/dark/system) in the nav.
- Data.kit — benchmark numbers copied from vtr/docs/BENCHMARK_RESULTS.md, nav and footer data.
- Landing.kit — hero, demo tabs + DemoFrame, proof points, feature rows, feature grid, targets, benchmark teaser, get started.
- Benchmarks.kit — PageHead/Block helpers, bars and tables from the report.
- Format.kit — VTR overview (data model, container, VTR/VDB boundary, coverage, APIs) from docs/SPEC.md and README.md.
- Docs.kit — docs template: sidebar, prose, on-page TOC with scroll-spy.
- Download.kit — download cards and build-from-source tabs.

Source of copy and structure: vtr/docs/volna-landing.html (existing landing page). Set DEMO_URL in Landing.kit to the hosted WASM viewer to embed the live demo; without it the frame shows real screenshots from the repo.
Download sizes/checksums are placeholders (no release is published).
