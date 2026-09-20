# AGENTS.md

Instructions for coding agents working in this repository. Read
[README.md](README.md) first: it is the canonical source for project purpose,
requirements, architecture direction, repository layout, documentation links,
and build commands. This file contains the ground rules for making changes.

## Scope and ownership

Add first-party code under the component that owns it. Do not work around a
missing abstraction in a consumer when the information belongs in a lower
layer. Keep shared benchmarks in `bench/`; keep standalone Python exporters in
`integrations/slang`; keep Verilator integration tooling in
`integrations/verilator` rather than in the pinned `ext/verilator` submodule
unless the backend itself must change.

For viewer work, follow the three Volna intents in README.md:

- Put viewer behavior, state, input interpretation, loading, viewport math,
  and toolkit-neutral painting in `volna-core` with headless tests.
- Treat GPUI `volna` as the feature frontend. Keep `volna-egui` building and
  preserve its current minimal behavior, but do not add feature parity work.
- Keep local and remote loading behind the same session interface and return
  the same immutable data objects. Protocols carry raw trace data only;
  presentation rules, VDB profiles, and annotations stay client-side.

MCP and other agent adapters must remain thin wrappers over toolkit-independent
core commands and query APIs and must preserve the VTR/VDB boundary.

## Automated, headless testing only

All testing and verification must be headless, fully automated, runnable
unattended in GitHub Actions CI, and invoked through checked-in commands. This
applies to unit and integration tests, native and browser UI tests,
accessibility checks, visual regressions, performance measurements, and
exploratory debugging.

- Do not open browsers or applications manually, use computer-use tools, or
  interact with a desktop to verify a change. Manual clicking, typing, and
  screenshot inspection are not valid test methods.
- Use `cargo test` and native headless harnesses for Rust/native behavior. Use
  automated JavaScript/TypeScript runners for browser and host integration;
  they must launch and drive their own headless browser and determine success
  with assertions.
- Test runners must provision fixtures, servers, and browser processes; use
  isolated temporary state; enforce timeouts; clean up; and return nonzero on
  failure. They must not depend on an open application, personal profile,
  interactive login, or user input.
- Dependencies and rendering backends must install and run on the selected
  GitHub Actions runner. Put platform coverage in an explicit CI matrix and
  report missing prerequisites as failures, not passes.
- Assert behavior, semantic accessibility data, rendered output, and measured
  performance programmatically as appropriate. Screenshots, traces, and logs
  may be saved only as diagnostics; manual review cannot be a passing gate.
- If a required check lacks an automated harness, implement or extend one.
  Older documentation describing manual verification does not override this
  rule.

Use the commands in README.md for normal checks. Viewer crates are explicit
workspace members but excluded from `default-members`; `cargo test -p
volna-core` provides platform-independent headless coverage, while
`volna/volna/check.sh` checks all viewer crates and the VS Code adapter.

## Design and implementation rules

- **Research first; prioritize clean formats and APIs.** Research relevant
  references before designing a solution. Reuse what they solve well, and
  record borrowed and rejected approaches in the appropriate design or
  rationale document. Check `docs/RATIONALE.md` before retrying an idea; when
  retesting one, record the new measurements there.
- **Prefer coherent breaking changes during the research phase.** Backward
  compatibility of the Rust API, C API/ABI, and file format is not required.
  Prefer a simpler design over compatibility indirection. Update affected
  implementations, callers, tests, and documentation together. Version
  formats so incompatible files fail clearly; old-version support is not
  required.
- **Fix missing abstractions at their owning layer.** Before adding a cache,
  side table, flag, or adapter workaround, check whether it duplicates data
  owned elsewhere. Add a small coherent query to the owning layer and update
  callers. Retain derived caches only for a demonstrated performance need,
  with explicit ownership and consistency rules.
- **Reassess design when scope changes.** If work expands across components,
  revisit earlier choices. Do not preserve a local workaround merely because
  it was already implemented or committed. A small diff is secondary to the
  simplest coherent cross-component design.
- **Design reader results for immutable access.** Readers serve inspection,
  not mutation. Share storage for repeated requests where useful rather than
  duplicating buffers to preserve independent mutability. Consumers that need
  transformations can explicitly copy.
- **Keep the public API small.** The C API is a one-to-one projection of the
  Rust API, with clear ownership and no hidden global state. VTR contains no
  presentation or design-source semantics beyond the documented log-site
  provenance exception; see README.md's “VTR/VDB boundary.”

## Format and documentation changes

A format change requires all of the following in the same change:

1. A new or changed code in `docs/SPEC.md`, with an appropriate format version
   when the change is incompatible.
2. Reader support and a round-trip test in `core/vtr/tests/`.
3. A rationale entry in `docs/RATIONALE.md`.
4. Regenerated fixtures where applicable.

Update `docs/SPEC.md`, `docs/RATIONALE.md`, API references, implementations,
and callers together. Keep documentation focused on current architecture,
behavior, requirements, and reproducible workflows. Use Git for history; do
not append dated progress reports, migration diaries, or per-change
verification records.

Keep all code, documentation, and benchmarks in this repository and make sure
they work from a clean checkout. Commit messages describe what changed and its
measured effect and contain no attribution trailers.

## Performance and benchmarks

- **Measure rather than assume.** Any encoding, compression, block/run-size,
  transform, or reader decode-path change must be A/B tested against the
  previous commit, normally via a worktree of `HEAD`, on benchmark files. Run
  the full suite before updating results and report size, write time, and read
  time together; do not trade one away silently.
- Every claim against README.md's efficiency requirements must be supported by
  the benchmark report.
- Simulator-integrated workloads link Verilator models against `libvtr.a`, but
  their Makefiles do not track the library. After changing the writer, delete
  `bench/workloads/gen/*/obj_vtr/{Vtop,rsa_tb}` before running the suite.
- Measurements vary by a few percent. Pin to P-cores (`taskset -c 0-7`), use
  best-of-N, and compare with a baseline binary in the same session.
