# Interned value validation

VTR 3.5 adds a recording-local value table and tagged references without changing
canonical payload schemas. The Rust and C++ implementations preserve typed
content, validate declaration order, and publish dependencies before runtime
sections. Volna resolves references into shared typed descriptions.

## Functional checks

The full Rust workspace suite and all 40 native CTest cases pass. Interoperability
runs both writers against both readers with stored, LZ4, and Zstandard data,
inline and threaded capture, background workers, recovery, and activity sidecars.
The fixture shares values across file metadata, hierarchy metadata, datatype
metadata, owner attributes, and embedded events. A second declaration chunk is
introduced after a flush. An independent Python decoder checks the chunks,
nested reference IDs, integer tags, bytes, and exact NaN bits.

Additional producer/reader scenarios cover repeated instruction descriptions,
changed machine code at the same address, inline execution masks, embedded stages,
relations, and interrupted files. Invalid self/forward references are rejected,
including nested values and attributes skipped during relation queries. Typed
identity distinguishes integer widths, signedness, signed zero, NaN payloads,
and ordered list contents.

Volna core, trace, and server suites pass, including loading and rendering the
existing all-types gallery, shared instruction labels, nested descriptions,
exact integer filtering/sorting, and remote transport. Strict Clippy passes for
both workspaces. The core and trace libraries also compile for WebAssembly with
a C compiler that supports that target. A retained-track test closes the session
and verifies that shared descriptions remain readable. Increasing one description
from 16 bytes to 64 KiB increases retained-byte accounting once, rather than once
for each of its 128 transaction references.

On macOS, GUI tests use `TMPDIR=/private/tmp` to avoid `/var` versus `/private/var`
path aliases. That run passes 140 of 142 tests. These two failures also occur on
an untouched Volna `e850732` / VTR `5d0e1c5` baseline under the same environment:

- `derived::tests::measurement_scope_controls_save_selected_markers_and_explicit_bounds`
- `derived::tests::sampling_picker_and_presets_keep_the_selected_capture`

Both concern formula-editor interaction; neither depends on interned attributes.

## Compressed size and new-feature costs

The [instruction comparison](../benchmarks/intern-values.md) writes 250,000
transactions with 1,024 interleaved descriptions and checks every decoded field
against independently generated expectations. File totals include the value
table, metadata, section headers, and indexes.

| Codec | Inline bytes/transaction | Interned bytes/transaction | Reduction |
|---|---:|---:|---:|
| LZ4 | 43.382 | 2.894 | 93.3% |
| Zstandard | 37.469 | 1.869 | 95.0% |

Writing and reading are faster for both codecs in this workload. The report
includes write/read time and peak RSS, and its adjacent JSON retains individual
measurements and build provenance. RSS comes from Linux `VmHWM`, avoiding the
parent-process high-water mark inherited by `getrusage` across fork/exec.

The complete Volna GPU example contains 80,000 instructions and 192,968 owners.
Sharing its `(pc, opcode)` descriptions reduces the file from 2,403,571 to
2,322,961 bytes while preserving its generated activity and provenance checks.
The small all-types gallery keeps each existing attribute visible and demonstrates
shared composite values; its table overhead adds about 60 bytes to the complete
trace. Interning is explicit because a small recording need not become smaller.

## Existing-workload regression checks

The rebuilt [full default report](../benchmarks/report.md) passes every workload,
format, read, and query oracle. All 490 table cells and six null-producer costs were compared with
both the previously committed report and a freshly built baseline; the complete
[comparison](../benchmarks/intern-regression/default-metrics.json) retains all
values. File sizes, bytes per observation, and file bytes per tick remain
unchanged for every format and workload.

The previously committed report used Linux x86-64; this validation used Linux
AArch64. Performance conclusions therefore use the fresh baseline on the same
host, compiler, CPU affinity 0, full workload sizes, and alternating variant order.
The [15-pair comparison](../benchmarks/intern-regression/report.md) includes every
measured metric, its [raw observations](../benchmarks/intern-regression/raw.json),
and both executable hashes.

| Workload | Write time change | Read time change |
|---|---:|---:|
| core | −6.5% | +2.9% |
| soc | −1.3% | −2.1% |
| idle | +0.5% | −1.6% |
| requests | +0.1% | +0.4% |
| many-generators | +1.4% | −0.2% |
| mixed | −1.3% | +0.6% |

All query medians change by less than +5%. The many-generator find-transaction
p95 increases from 5.782 to 8.310 ms (+43.7%). A focused
[60-pair rerun](../benchmarks/intern-regression/query-tail.json), with the same
binaries and CPU affinity, checks 720 probes per variant: baseline p50/p95 are
4.645/4.921 ms, versus candidate 4.557/4.870 ms. The spike does not reproduce.

An additional [15-pair diagnostic](../benchmarks/intern-regression/nohuge-report.md)
disables transparent huge pages per process with
`prctl(PR_SET_THP_DISABLE, 1, 0, 0, 0)`, without changing system settings. Its
write medians range from −2.7% to +1.6% and read medians from −0.7% to +1.2%.
The many-generator find p95 changes by +3.3%; core point-query p95 increases by
0.083 ms (+9.2%). Small component timings vary: requests close increases from
1.06 to 1.42 ms in the normal run, and decreases from 1.31 to 1.19 ms in the
diagnostic. Total write time remains within 0.1% in both runs.

RSS comparisons show page-allocation variation: mixed find-transaction peaks
at 4.89/6.81 MiB normally, versus 4.89/4.83 MiB with huge pages disabled.
Many-generator full-read peaks are 7.75/7.83 MiB normally and 5.97/5.91 MiB
in the diagnostic. No repeatable performance regression was identified within
these measurements; they do not establish a bound for every workload.

## Volna loading and retained memory

A macOS release-build comparison loads the first GPU instruction track, checks
10,000 rows and their identity/time checksum, and measures retained-byte
accounting separately. Nine shuffled rounds compare the untouched baseline,
the candidate reading equivalent inline data, and the candidate reading the
interned example. The fresh inline trace has an identical body after the format
header. The [results](../benchmarks/intern-regression/volna-loading.json) include
the source, runner, executable hashes, and every observation.

| Variant | Open (ms) | Load (ms) | Account (ms/call) | Accounted bytes |
|---|---:|---:|---:|---:|
| Baseline | 4.926 | 86.596 | 0.346 | 26,131,165 |
| Candidate, inline | 5.018 | 85.201 | 0.327 | 26,131,165 |
| Candidate, interned | 4.878 | 78.479 | 0.367 | 24,210,588 |

Interned data loads about 9% faster and retains about 7% fewer accounted bytes.
Shared-allocation accounting adds about 0.02 ms per call relative to the
baseline. These bytes describe the loaded track's accounting, rather than
process RSS; declaration-cache memory is accounted separately. The inline
comparison shows no slowdown from the new accounting path in this workload.

## Remote sharing

Volna protocol 17 transfers each metadata or track object's shared values once,
in dependency order, followed by references to that table. IDs are local to the
transport object. Decoding validates backward references without expanding them
into recursive trees. Shared table storage is admitted once and its reservation
follows every independently retained generator using that object.

A server-process integration test opens metadata and loads 128 transactions
under a 1 MiB object limit and 4 MiB memory budget. All transactions, embedded
events, stages, and relations share a 64 KiB description, an 80-level reference
chain, and a branching graph. The result preserves types, dynamic masks, and
shared identity. Retaining a generator after closing the session keeps both its
description and memory reservation alive; dropping it releases the reservation.
Malformed self/forward references fail decoding and release private storage.

The [remote comparison](../benchmarks/intern-regression/remote-sharing.json)
retains the reproduction source, executable hashes, all measurements, and a
reverse patch for the pre-fix baseline. A 572-byte branching-graph recording
previously expanded to 9,961,734 bytes of metadata; it now requires 822 bytes.
The 64- and 80-level reference chains that previously failed remote decoding
now load successfully. These are serialized body sizes before packet compression.

Nine shuffled release-build rounds load the same 10,000-row GPU track and verify
its row/time checksum. Encoding includes preparation, the size prepass, and
serialization; decoding includes cooperative validation and index construction.

| Input | Encode before/after (ms) | Decode before/after (ms) | Serialized bytes before/after | Admitted bytes before/after |
|---|---:|---:|---:|---:|
| Inline | 2.716 / 3.160 | 64.168 / 64.208 | 10,031,814 / 10,031,822 | 43,380,717 / 43,380,725 |
| Interned | 2.649 / 2.955 | 67.181 / 57.546 | 10,201,814 / 9,146,997 | 44,750,605 / 41,360,988 |

Discovering the table adds about 0.44 ms to inline encoding in this workload;
its body and retained allocation charges grow by the fixed eight-byte table
header and generator ownership field, respectively. The interned trace reduces
serialized bytes by 10.3%, admitted bytes by 7.6%, and decoding time by 14.3%.
Encoding adds about 0.31 ms. This is the measured cost of preserving shared
graphs across transport rather than expanding their occurrences.

## Shared-value comparisons and filtering

Volna compares shared graphs with an explicit work stack and a cache of node
pairs already compared. Inline scalar comparisons keep their allocation-free
path. Ordinary value equality retains shared-wrapper boundaries and Rust float
equality: NaNs compare unequal and signed zeros compare equal. Sorting resolves
references and retains total float ordering; remote relation validation resolves
references and compares float bits. Cooperative validation can yield during graph
traversal and long byte comparisons. Filtering does not revisit shared subgraphs within an attribute, including
when no leaf matches.

An application regression opens 64 transactions with a 64-level branching
description, sorts both directions, and checks stable row identities through
infinities, signed zeros, and distinct NaNs. It then applies nonmatching, matching,
and map-key filters. Separate recordings exercise content equality, changed
leaves, ordinary NaN/signed-zero behavior, and 1,024 nested shared levels.
Repeated relation details retain bitwise float validation on independent graphs.

The [comparison measurements](../benchmarks/intern-regression/graph-comparison.json)
include release executable hashes, source, a baseline reverse patch, and all
observations from three shuffled before/after rounds on the same macOS host.
Sorting uses the real headless application and includes its 1 ms completion
polling interval; these small after-times are query completion latencies, rather
than isolated comparator timings.

| Shared graph nodes | Recording bytes | Rows | Sort before (ms) | Sort after (ms) |
|---|---:|---:|---:|---:|
| 9 | 910 | 64 | 2.573 | 1.292 |
| 17 | 958 | 64 | 199.993 | 1.285 |
| 21 | 983 | 64 | 2,829.501 | 1.283 |

Comparing a public shared value with its clone changes from 16.285 ms to 0.002 ms
for a 23-node branching graph. A nine-node comparison changes from 0.001 ms to
0.004 ms: the small fixed memoization cost replaces exponential repeated work.

## Shared-value lifetime

Individually shallow table entries can form a deep predecessor chain. Shared
ownership must release that chain without recursive destruction when its last
handle disappears. Volna's `SharedValue` keeps its reference-counted owner private
and uses `Arc::into_inner` to obtain the value for iterative release. Remaining
list/map iterators retain their existing buffers while the release loop drains
them. The same owner type is used in caches and retained attributes; reclamation
does not depend on the session's lifetime or the thread dropping the last clone.

The original 401,128-byte recording with 100,000 linked lists now opens and closes
successfully. Before the ownership change, closing it aborted with a stack
overflow. A process-isolated regression generates a 100,000-level recording and
checks local session closure, a transaction description retained after session
closure, and remote metadata/track closure through the real server. Its retained
remote generator stays charged until release, then returns its memory budget to
zero. Eight concurrent owners also release a 100,000-level mixture of lists,
maps with sibling fields, and shared aliases successfully.

The [ownership comparison](../benchmarks/intern-regression/shared-ownership.json)
retains the reproducer, before/after results, benchmark source, executable hashes,
and a reverse patch for the baseline. Nine shuffled release-build rounds use the
same 10,000-row GPU track and verify its identity/time checksum. Metadata and
track body SHA-256 hashes, body sizes, and retained-byte accounting match exactly
between implementations for both inline and interned traces. VTR 3.5 and Volna
protocol 17 retain their encodings.

| Input | Open before/after (ms) | Load before/after (ms) | Release track before/after (ms) | Close session before/after (ms) |
|---|---:|---:|---:|---:|
| Inline | 4.870 / 4.861 | 83.505 / 84.497 | 3.308 / 3.185 | 0.065 / 0.068 |
| Interned | 4.781 / 4.730 | 76.810 / 77.184 | 2.691 / 2.799 | 0.074 / 0.079 |

In this run, interned loading changes by +0.5% and releasing its track adds
0.11 ms. Retained charges remain 26,131,165 bytes for inline data and 24,210,588
bytes for interned data. These observations measure ordinary loading and release;
the deep-chain regression separately verifies safe teardown without a nesting cap.

## End-to-end revalidation

The 2026-11-11 [revalidation](../benchmarks/intern-regression/final-validation.json)
retains the source hashes, test results, baseline failure evidence, and fresh
benchmark observations. All 40 native interoperability tests pass. On macOS,
all 180 VTR Rust target tests, four doc tests, formatting, and strict workspace
Clippy pass. Volna's core, trace, and server suites pass 1,021 tests; the broader
all-target run passes 1,167 tests, with eight manual profiling tests ignored.
Volna doc tests, formatting, and strict workspace Clippy also pass.

The fresh WebAssembly viewer and converter build successfully. Documentation
checks pass and verify 613 local links across 15 pages. All 13 browser-support
tests, browser import/conversion scenarios, and million-row sorting/filtering
scenarios pass, including cancellation, responsive frames, selection, and memory
refusal. The extension passes 28 Node tests, five packaging tests, and its checks.

The expanded checks expose additional failures that also reproduce on untouched
Volna `e850732` / VTR `5d0e1c5`, alongside the two formula-editor failures above:

- The derived example's workspace-regeneration test reports a missing
  `transaction` field in its generated waveform panel.
- The browser site test's occupancy-navigation assertion observes record 2788
  replacing record 109. Both builds produce the same selection and history state.
- Linux AArch64's Rust scalar-format oracle reports 65 character-byte mismatches.
  Strict Linux Clippy also reports two existing `c_char` grouping diagnostics.
  The remaining Linux Rust targets and doc tests pass when that oracle is excluded.

Both complete benchmark suites pass again at their default sizes and three
repeats. The default suite takes 51.59 seconds; all 490 metric cells and six null
producer costs were compared with the committed report and previous candidate.
Every file-size metric remains unchanged. VTR write/read medians are lower than
the previous candidate; RSS still varies with page allocation, including mixed
point-query peak RSS of 5.62 versus 7.25 MiB. The earlier paired measurements and
huge-page diagnostic remain the controlled regression comparison. Updated
[default](../benchmarks/report.md) and [instruction](../benchmarks/intern-values.md)
reports retain the same compressed-size improvements.

Reproduce from the VTR repository root:

```sh
cmake -S vtr-cpp -B build/cpp -DCMAKE_BUILD_TYPE=Release
cmake --build build/cpp --parallel
ctest --test-dir build/cpp --output-on-failure
cargo test --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
python3 benchmarks/build.py
python3 benchmarks/run.py
python3 benchmarks/intern_values.py
python3 benchmarks/compare.py /path/to/baseline/benchmark --repeats 15
```

The benchmark scripts require Linux for affinity and process memory measurement.
Keep the baseline executable and its build provenance before rebuilding the
candidate. Regenerate older VTR assets; readers accept only the current format.
