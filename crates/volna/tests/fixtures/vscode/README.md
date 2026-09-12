# VS Code theme fixtures

The four `.txt` snapshots contain resolved webview CSS variables from VS Code
1.137: Dark Modern, Light Modern, Default High Contrast, and Default High
Contrast Light. They were reused from the sibling `claude-theme` workspace;
the snapshot headers record the source and theme kind. These are real resolved
palettes, alongside the deliberately sparse and mixed-surface synthetic cases.

`palettes.rs` is generated from these snapshots through the production JavaScript
mapping so native tests consume typed palette inputs without a runtime parser or
new dependency. The JavaScript test checks that it remains synchronized:

```sh
node --test crates/volna/vscode-ext/theme.test.mjs
UPDATE_THEME_FIXTURES=1 node --test crates/volna/vscode-ext/theme.test.mjs
```

Use the second command only after intentionally changing a snapshot or mapping.
The native viewer test also captures all four palettes with selection, cursor,
zoom, marker and an open format menu, asserting unchanged viewer diagnostics.
