# Compatibility paths

First-party libraries live in [`core/`](../core/README.md), tools in
[`tools/`](../tools/README.md) and the UI in [`volna/`](../volna/README.md).

The pinned `ext/surfer` submodule refers to `crates/vtr`,
`crates/vtr-vdb` and `crates/vtr-capi/include`. Relative symlinks here preserve
those dependencies without copying code. They are
not additional workspace members. Use the canonical component paths for new
development.
