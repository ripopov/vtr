# Conversion progress

`vtr-progress` defines work counters and versioned conversion progress messages.
It has no trace readers or process execution. Converters update counters; CLI,
native, and browser hosts render snapshots. Totals apply to the current phase,
not the whole conversion. Unknown totals remain indeterminate.

`vtr convert --progress=json` emits newline-delimited `Snapshot` objects on stdout.
Each object carries `version` (currently 1), `phase`, `completed`, optional `total`,
`input_bytes`, and `output_bytes`. Sizes are bytes on disk; output size measures
the private output file before publication and the destination after success.
`complete` is emitted only after publication succeeds. Diagnostics go to stderr.

```json
{"version":1,"phase":"writing","completed":42,"total":100,"input_bytes":268435456,"output_bytes":40579891}
```

Readers reject unsupported versions, invalid counters, incomplete lines, and
records longer than 4096 bytes. Update `VERSION` when changing this contract.

Synchronous hosts can use `Reporter::with_observer` to receive phase changes and
coalesced counter updates on the calling thread. Known totals divide the phase
into about 1,000 reporting steps, keeping large timestamp units from triggering
a callback for every sample. Callbacks run without the progress lock.
Browser workers post these snapshots to the page, throttling
counter messages to 100 ms while forwarding every phase change. Their byte sizes
measure the original input and the VTR bytes emitted to memory; `complete`
follows writer finalization.
