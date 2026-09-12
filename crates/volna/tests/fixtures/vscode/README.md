# VS Code theme fixtures

The four `.txt` snapshots contain resolved webview CSS variables from VS Code
1.137: Dark Modern, Light Modern, Default High Contrast, and Default High
Contrast Light. The snapshot headers record their provenance and theme kind.

Native unit tests and the Metal viewer harness pass these raw snapshots directly
through `theme::vscode::host_palette`, the same Rust adapter used by the webview.
There are no generated fixtures to synchronize. The mixed palette in
`../custom-palette.json` also exercises the host-neutral JSON parser in both tests.

The viewer harness captures the palettes with selection, cursor, zoom, a marker
and an open format menu, asserting unchanged viewer diagnostics. JavaScript tests
cover snapshot transport and observer lifecycle independently of colour mapping.
