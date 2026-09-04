// Freestanding support for the C910 CoreMark image (VTR benchmark harness).
// Replaces vendor/.../tests/lib/clib/vtimer.c. `get_vtimer` reads the M-mode
// cycle counter instead of the `time` CSR (no CLINT mtime in this TB), and
// `sim_end` is a no-op: the run is terminated by crt0's __exit sequence, which
// the testbench detects.

int get_vtimer(void) {
  unsigned long c;
  __asm__ volatile("csrr %0, mcycle" : "=r"(c));
  return (int)c;
}

unsigned long get_minstret(void) {
  unsigned long c;
  __asm__ volatile("csrr %0, minstret" : "=r"(c));
  return c;
}

void sim_end(void) {}
