# Container dependencies

`unordered_dense/` vendors the unmodified `map` and `set` headers from
[`ankerl::unordered_dense` v5.3.2](https://github.com/martinus/unordered_dense/releases/tag/v5.3.2),
revision `ddd49116e63be7345b448351cad153653a712fdb` (2026-10-09), under the
[MIT license](unordered_dense/LICENSE).
[UPSTREAM.json](unordered_dense/UPSTREAM.json) records the source revision and
SHA-256 of every upstream file. Update headers, license, and manifest together.
The benchmark build verifies these hashes. Builds require no dependency download.

The dependency is private to the implementation: public C and C++ headers do not
expose its types or require its headers. The installed library includes its
license under `share/licenses/vtr/unordered_dense/`.

## Selection (2026-10-19)

| Candidate | Relevant properties | Decision |
|---|---|---|
| [unordered_dense](https://github.com/martinus/unordered_dense) | Header-only; contiguous values; SIMD index; heterogeneous lookup; map and set | Use for sparse keys and iteration. Two headers, no external dependencies. |
| [Boost.Unordered](https://www.boost.org/doc/libs/latest/libs/unordered/doc/html/unordered/intro.html) | Header-only flat and node tables; SIMD lookup; node variant retains addresses across rehash | Strong lookup-oriented alternative, but vendoring requires its Boost dependency closure. |
| [Abseil Swiss tables](https://abseil.io/docs/cpp/guides/container) | Flat and node layouts; heterogeneous lookup | Requires compiled Abseil support; does not meet the standalone header-only constraint. |
| [GTL](https://github.com/greg7mdp/gtl) | C++20 header-only Swiss tables and B-trees; successor recommended by parallel-hashmap's maintainer | Useful alternative for large or concurrent tables; VTR's current producer is single-threaded. |
| [tsl::robin_map](https://github.com/Tessil/robin-map) | Header-only Robin Hood tables; backward-shift erase | Viable compact fallback; no need to vendor a second hash implementation before an end-to-end win is demonstrated. |

There is no universal fastest table. The author's
[September 2026 comparison](https://martin.ankerl.com/2026/09/04/which-hash-map/)
shows lookup and iteration favoring different layouts. Those results guide
candidate selection; they do not predict VTR's speedups. VTR measurements are in
[the container tuning report](../../benchmarks/containers.md).

Dense signal IDs use indexed arrays instead of hashing. Duplicate detection uses
one flat candidate chain, avoiding a vector allocation per distinct hash.
String interning owns its keys and accepts `string_view` probes, allocating a key
only on a miss. Dictionary keys view immutable input bytes for the duration of
encoding. Hash matches still require exact equality before aliasing signals.

Dense-map insertion and erasure invalidate iterators and references. Keep them
local to an operation; the history cache returns independently owned
`shared_ptr` values. No hash-table iteration order defines file IDs or record
order. Ordered maps in the public simulator helpers retain their existing
diagnostic ordering.
