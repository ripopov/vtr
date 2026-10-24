# Comments and documentation

Express ideas directly through clear names, types, control flow, and meaningful
tests. Improve confusing code before adding a comment to explain it. Avoid
comments that restate the implementation. Keep module descriptions brief and at
the top of the module.

Use brief comments when code cannot convey the reason for a constraint or
decision: external requirements, non-obvious tradeoffs or rejected alternatives,
and subtle safety invariants that a maintainer might otherwise remove. Explain
why the constraint or decision matters, rather than narrating how the code works.

Write comments and documentation for maintainers who have never seen the prompts
or conversation. Keep rationale self-contained; omit prompt references,
acknowledgments, and change narration. Before finishing, review added or edited
comments and documentation: would they still help someone understand or safely
change the code if the conversation disappeared?

# Documentation layout

Keep project documentation in `README.md` and `docs/`. Component directories
contain implementation and fixtures; describe each component briefly in the root README.

# Testing

Do not add trivial unit tests that merely mirror static declarations or
configuration. Focus tests on meaningful behavior, edge cases, and nontrivial invariants.

Whenever the VTR implementation or trace format changes, rebuild and rerun the
native interoperability tests and full default benchmark suite from the
repository root:

```sh
cmake -S vtr-cpp -B build/cpp -DCMAKE_BUILD_TYPE=Release
cmake --build build/cpp --parallel
ctest --test-dir build/cpp --output-on-failure
python3 benchmarks/build.py
python3 benchmarks/run.py
```

Compare the generated `benchmarks/report.md` with the previously committed
report for every workload and every measured metric, including write/read
speed, file size, memory, query latency, and producer costs. Validate that there
are no unexpected regressions. Investigate regressions before accepting the
change; fix unintended regressions and document the rationale and evidence for
any intentional tradeoff. Use comparable hardware, CPU affinity, and benchmark
settings, and rerun when measurement noise makes a comparison inconclusive.
Do not substitute smoke runs or partial suites for this check. Include the
updated report with the implementation or format change when staging or
committing it.

# Staging and commits

Stage completed changes. Commit only when explicitly requested by the user.
Include only task-related changes; preserve unrelated work and its staging state.

# Temporary plans

`./atemp` is a temporary directory for plans. Never commit this directory or its contents.

# Compatibility

VTR is a research project. Do not add backwards-compatibility machinery
for old assets or interfaces. When a
protocol, serialization format, or other API layer changes, bump its version
as needed and ignore or regenerate assets produced by older versions.
