/* Regenerate from the repository root:
 * cc volna/volna-core/tests/fixtures/fst_values.c \
 *   ext/libfstwriter/integration_test/verilator_share/gtkwave/fstapi.c \
 *   -Iext/libfstwriter/integration_test/verilator_share/gtkwave \
 *   $(pkg-config --cflags --libs liblz4 zlib) -o /tmp/volna-fst-values
 * /tmp/volna-fst-values volna/volna-core/tests/fixtures/values.fst
 * /tmp/volna-fst-values volna/volna-core/tests/fixtures/values-wrapped.fst wrap
 */
#include "fstapi.h"
#include <assert.h>
int main(int argc, char **argv) {
    assert(argc == 2 || argc == 3);
    fstWriterContext *w = fstWriterCreate(argv[1], 1);
    assert(w);
    if (argc == 3) fstWriterSetRepackOnClose(w, 1);
    fstWriterSetPackType(w, FST_WR_PT_ZLIB);
    fstWriterSetTimescale(w, -12);
    fstWriterSetScope(w, FST_ST_VCD_MODULE, "top", "values");
    fstHandle text = fstWriterCreateVar(w, FST_VT_GEN_STRING, FST_VD_IMPLICIT, 0, "bytes", 0);
    fstHandle real = fstWriterCreateVar(w, FST_VT_VCD_REAL, FST_VD_IMPLICIT, 8, "real", 0);
    fstHandle logic = fstWriterCreateVar(w, FST_VT_VCD_WIRE, FST_VD_INPUT, 9, "logic", 0);
    fstWriterCreateVar(w, FST_VT_VCD_WIRE, FST_VD_OUTPUT, 9, "alias", logic);
    fstHandle event = fstWriterCreateVar(w, FST_VT_VCD_EVENT, FST_VD_IMPLICIT, 1, "event", 0);
    fstWriterSetUpscope(w);
    const unsigned char bytes[] = { 'a', 0, 255, '\\', '\n', 128 };
    double r = 1.25;
    fstWriterEmitTimeChange(w, 5);
    fstWriterEmitVariableLengthValueChange(w, text, bytes, sizeof(bytes));
    fstWriterEmitValueChange(w, real, &r);
    fstWriterEmitValueChange(w, logic, "01xzuwlh-");
    fstWriterEmitValueChange(w, event, "1");
    fstWriterEmitTimeChange(w, 10);
    fstWriterEmitVariableLengthValueChange(w, text, "", 0);
    r = -2.5;
    fstWriterEmitValueChange(w, real, &r);
    fstWriterEmitValueChange(w, logic, "111111111");
    fstWriterEmitValueChange(w, event, "1");
    fstWriterEmitTimeChange(w, 20);
    fstWriterClose(w);
    return 0;
}
