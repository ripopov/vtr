# Interned instruction values

Host: Linux-7.0.14-linuxkit-aarch64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Transactions per trace: 250,000.

Each trace replays the same interleaved sequence of 1,024 instruction descriptions. Each description contains an instruction pointer, 32 machine-code bytes, a mnemonic, and an execution pipe. A dynamic execution mask stays inline. Both variants use the same API, compression, and buffer settings. Write time includes table construction, capture, encoding, compression, and close. Read time includes opening, decoding, resolving, and independently checking every instruction and mask. File size includes all declaration tables, metadata, section headers, and indexes. Fresh processes measure peak RSS; medians report time and the maximum reports RSS. Variant order alternates across repeats. OS page cache is warm after writing.

| Codec | Representation | Bytes/transaction | File bytes | Write (ms) | Read (ms) | Write RSS (MiB) | Read RSS (MiB) |
|---|---|---:|---:|---:|---:|---:|---:|
| LZ4 | inline | 43.382 | 10,845,473 | 163.25 | 124.74 | 7.66 | 16.79 |
| LZ4 | interned | 2.894 | 723,516 | 30.32 | 38.38 | 7.05 | 5.43 |
| ZSTD | inline | 37.469 | 9,367,185 | 175.87 | 138.92 | 8.13 | 15.54 |
| ZSTD | interned | 1.869 | 467,367 | 23.61 | 43.21 | 6.87 | 7.50 |
