---
name: volna-design
description: Use this skill when creating or restyling any HTML page in the VTR repository (the Volna website, docs pages, proposal and demo pages) or a throwaway mock of Volna-branded UI. Contains the design direction, content voice, tokens, v- component classes, brand assets and the website UI kit.
---

This is the shared design skill for any agent working in this repository. Read [readme.md](readme.md) in this directory first, then the files it points to. It is the source of truth; its Ownership section says which layer is maintained and which is a frozen reference. Resolve design-system paths relative to this directory and repository paths relative to the repository root.

For pages in this repository: link `styles.css` by a relative path (for example `design-system/styles.css` from `docs/`), set `<html data-theme="dark|light">`, compose the `v-` classes from `components/components.css` in static HTML, and take structure from `ui_kits/website/` and the component `.jsx` files without adding React. Reference fonts, icons and the app icon from `volna/`; never copy them. Numbers must come from the VTR benchmark report and imagery must be real product screenshots. Extend tokens and classes here rather than in a page, add a specimen to `gallery.html` for every new class, and run the guardrails in readme.md's Guardrails section after changing this directory or an adopting page.

For throwaway mocks outside the repository, copy the assets you need next to the mock.
