# slang-server for Surfer's source tile

The `ext/slang-server` submodule pins a commit from the
[`vtr` branch](https://github.com/ripopov/slang-server/tree/vtr) of
[slang-server](https://github.com/hudson-trading/slang-server), the language
server built on the [slang](https://github.com/MikePopoloski/slang) SystemVerilog
frontend. The branch adds what a waveform viewer needs and upstream does not
provide:

- `textDocument/semanticTokens/full`: every token of a file classified from the
  shallow compilation (keywords, comments, literals, macros, and resolved symbol
  kinds with `declaration`, port direction and `clock` modifiers). Clocks are the
  edge-sensitive event signals a process never reads as data.
- `slang.getInactiveGenerateRanges`: the generate blocks of a file that one
  elaborated instance leaves uninstantiated, so the viewer can dim them per
  instance.

Surfer (`ext/surfer`) starts the server automatically when it opens a VTR
recording whose VDB companion carries an `elaboration` record (written by the
pinned Verilator's `--trace-vtr` and by `tools/vdb/export.py`). The record's
files, include directories, defines and top module become a build file for
`slang.setBuildFile`, so the server elaborates exactly the design that was
simulated. See the Surfer chapter `ext/surfer/docs/html/source-code.html` and
[VDB_RTL.md](../../docs/VDB_RTL.md).

## Build

Requires CMake 3.28+, a C++20 compiler (clang 17+ or GCC 11+) and Python 3.

```sh
git submodule update --init --recursive ext/slang-server
integrations/slang-server/build.sh bench/build/slang-server/install
```

The script prints the installed binary path. Put it on `PATH`, name it in
Surfer's config (`[slang] server = "..."`), or set `SURFER_SLANG_SERVER`.
The server's own tests cover the additions:

```sh
cmake --build bench/build/slang-server/build --target server_unittests
bench/build/slang-server/build/bin/server_unittests "Semantic tokens*,Inactive generate*"
```

## Surfer tests without the server

Surfer's snapshot tests replay recorded sessions
(`ext/surfer/examples/verilator/*.slang.json`) through the same client code, so
`cargo test -p libsurfer` needs no C++ toolchain. After changing the server, the
example designs or the recorded scenarios, regenerate the recordings against a
real binary:

```sh
cd ext/surfer
SURFER_SLANG_SERVER=$PWD/../../bench/build/slang-server/install/bin/slang-server \
  cargo test -p libsurfer --lib record_slang_sessions -- --ignored
```
