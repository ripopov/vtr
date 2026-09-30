// check <file.vtr>: prints recovered flag, value changes, log records, last time and fatal messages as JSON.
#include "vtr.h"
#include <cstdio>
#include <cstdint>
#include <string>
static uint64_t n_changes = 0, t_last = 0;
static int on_change(void *, uint64_t t, uint32_t, const vtr_signal_value *) { ++n_changes; if (t > t_last) t_last = t; return 0; }
struct Ctx { const vtr_reader *r; std::string fatal; };
static int on_log(void *u, const vtr_log_rec *rec) {
    Ctx *c = static_cast<Ctx *>(u);
    char buf[256];
    vtr_log_rec_format(c->r, rec, buf, sizeof buf);
    c->fatal = buf;
    return 0;
}
int main(int argc, char **argv) {
    vtr_reader *r = vtr_reader_open(argv[1]);
    if (!r) { printf("{\"open\":false,\"error\":\"%s\"}\n", vtr_last_error()); return 1; }
    vtr_meta m{};
    vtr_reader_meta(r, &m);
    vtr_reader_for_each_change(r, 0, UINT64_MAX, on_change, nullptr);
    Ctx c{r, ""};
    vtr_reader_visit_log(r, VTR_NONE, VTR_NONE, 5, 0, 0, on_log, &c);
    printf("{\"open\":true,\"recovered\":%d,\"changes\":%llu,\"logs\":%llu,\"last_time\":%llu,\"blocks\":%u,\"fatal\":\"%s\"}\n", m.recovered,
           (unsigned long long)n_changes, (unsigned long long)m.log_count, (unsigned long long)t_last, m.signal_block_count, c.fatal.c_str());
    vtr_reader_close(r);
}
