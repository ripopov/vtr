# VTR C API

`vtr-capi` builds `libvtr`, a C ABI over the workspace's `vtr` trace reader and
writer. [`include/vtr.h`](include/vtr.h) is the API reference. The other public
headers provide C++17 logging and transaction helpers and a SystemVerilog DPI
front end. `vtr-guard` provides optional Linux crash and stop handling.

## Build and link

On Linux, run this from the workspace root after cloning Volna (no submodules
or desktop development libraries are needed):

```sh
./vtr-capi/install.sh --target-dir /tmp/vtr-build --prefix /tmp/vtr-install
```

Use the path to `install.sh` in a submodule when embedding VTR. Both directories
are required and may be absolute paths outside the checkout; relative paths are
resolved from the calling directory. The command builds `vtr-capi` with Cargo's
locked dependency set and installs `lib/libvtr.a`, the public C and C++ headers
in `include/`, and `lib/pkgconfig/vtr.pc`. It defaults to release; pass `--debug`
for a debug build. Pass `--with-dpi` to install the optional `vtr_trace.sv` and
`vtr_trace_dpi.hpp` helpers. Pass `--private-heap` when the crash guard must
work while the application's allocator is locked by a fault. Use
`--toolchain NAME` to select an installed rustup toolchain;
`RUSTUP_TOOLCHAIN` also works. The repository's `rust-toolchain.toml` is used by
default.

The pkg-config file includes the native libraries needed to link the static
archive on the selected Linux toolchain. Consumers can use the installed files
without knowing Cargo's output layout:

```sh
export PKG_CONFIG_PATH=/tmp/vtr-install/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}
cc -std=c99 $(pkg-config --cflags vtr) smoke.c \
   $(pkg-config --static --libs vtr) -o smoke
./smoke trace.vtr
```

For example, save this as `smoke.c` outside the Volna checkout:

```c
#include "vtr.h"
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
    const char *path = argc > 1 ? argv[1] : "trace.vtr";
    vtr_writer *w = vtr_writer_create(path, NULL);
    if (!w) { fprintf(stderr, "%s\n", vtr_last_error()); return 1; }
    uint32_t top = vtr_writer_add_scope(w, VTR_NONE, "top", VTR_SCOPE_MODULE, NULL);
    uint32_t node, signal;
    int rc = top == VTR_NONE ? VTR_ERR_INVALID :
        vtr_writer_add_var(w, top, "clk", VTR_VAR_WIRE, VTR_DIR_INPUT,
                           VTR_SIGNAL_BITS, 1, 4, &node, &signal);
    if (rc == VTR_OK) rc = vtr_writer_set_time(w, 0);
    if (rc == VTR_OK) rc = vtr_writer_emit_bit(w, signal, VTR_LOGIC_0);
    if (rc == VTR_OK) rc = vtr_writer_set_time(w, 5);
    if (rc == VTR_OK) rc = vtr_writer_emit_bit(w, signal, VTR_LOGIC_1);
    if (rc != VTR_OK) fprintf(stderr, "%s\n", vtr_last_error());
    int close_rc = vtr_writer_close(w);
    if (close_rc != VTR_OK) fprintf(stderr, "%s\n", vtr_last_error());
    if (rc != VTR_OK || close_rc != VTR_OK) return 1;

    vtr_reader *r = vtr_reader_open(path);
    if (!r) { fprintf(stderr, "%s\n", vtr_last_error()); return 1; }
    vtr_value_buf *value = vtr_value_buf_new();
    uint32_t found = VTR_NONE;
    int ok = value && vtr_reader_find_signal(r, "top.clk", '.', &found) == VTR_OK &&
             vtr_reader_value_at(r, found, 5, value) == VTR_OK &&
             strcmp(vtr_value_buf_ascii(value), "1") == 0;
    if (!ok) fprintf(stderr, "trace read failed: %s\n", vtr_last_error());
    vtr_value_buf_free(value);
    vtr_reader_close(r);
    return ok ? 0 : 1;
}
```

## Ownership and errors

The caller owns opaque handles and frees them with their matching close or
free function. `vtr_writer_close` always consumes its writer, even on error.
Status functions return `VTR_OK` or a `VTR_ERR_*` code; constructors return
`NULL` or `VTR_NONE` on failure. Read `vtr_last_error()` on the failing thread
immediately after a failure. Success does not clear the last error. Input
strings are UTF-8 and copied by writer calls; reader results and callback
pointers have the lifetimes described in `vtr.h`.

Run the C and C++ integration tests with
`cargo test -p vtr-capi --locked` on Linux.
