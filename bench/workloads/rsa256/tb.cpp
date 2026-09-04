// Verilator testbench for the RSA256 design from ext/libfstwriter (MIT), used to
// generate a real RTL FST workload. Usage: rsa_tb <out.fst> [cycles]
#include "VRSA_tb.h"
#include <cstdlib>
#include <memory>
#include <verilated.h>
#include <verilated_fst_c.h>

int main(int argc, char **argv) {
    if (argc < 2) return 1;
    Verilated::commandArgs(argc, argv);
    long cycles = argc > 2 ? atol(argv[2]) : 200000;
    std::unique_ptr<VRSA_tb> tb(new VRSA_tb);
    std::unique_ptr<VerilatedFstC> tfp(new VerilatedFstC);
    vluint64_t t = 0;
    tb->clk = 0;
    tb->rst_n = 0;
    Verilated::traceEverOn(true);
    tb->trace(tfp.get(), 99);
    tfp->open(argv[1]);
    while (t < 20) { tb->clk = !tb->clk; tb->eval(); tfp->dump(t); t++; }
    tb->rst_n = 1;
    for (long i = 0; i < cycles * 2; i++) { tb->clk = !tb->clk; tb->eval(); tfp->dump(t); t++; }
    tfp->close();
    tb->final();
    return 0;
}
