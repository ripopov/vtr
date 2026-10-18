#include "vtr.h"
#include "vtr.hpp"
#include <chrono>
#include <csignal>
#include <cstdlib>
#include <iostream>
#include <string>
#include <sys/resource.h>
#include <thread>
int main(int argc, char **argv) {
  if (argc < 3)
    return 2;
  try {
    std::string mode = argv[1], path = argv[2];
    if (mode == "check") {
      vtr::Reader r(path);
      auto e = r.ending();
      auto history = r.load_signal(0);
      std::cout << unsigned(e.kind) << ' ' << e.status << ' ' << e.sealed << ' '
                << history->times.size() << '\n';
      return 0;
    }
    rlimit limit{0, 0};
    setrlimit(RLIMIT_CORE, &limit);
    vtr_guard_options options;
    vtr_guard_options_default(&options);
    options.stop_grace_ms = 100;
    options.deadline_ms = 500;
    if (vtr_guard_install(&options, nullptr))
      return 3;
    auto *w = vtr_writer_create(path.c_str(), nullptr);
    if (!w || vtr_guard_watch(w))
      return 3;
    auto scope = vtr_writer_add_scope(w, VTR_NONE, "top", 0, nullptr);
    uint32_t signal;
    if (vtr_writer_add_var(w, scope, "value", VTR_VAR_WIRE, 0, VTR_SIGNAL_BITS,
                           32, 2, nullptr, &signal))
      return 3;
    for (uint64_t i = 0; i < 100; ++i)
      if (vtr_writer_set_time(w, i) || vtr_writer_emit_u64(w, signal, i))
        return 3;
    if (mode == "exit")
      std::exit(7);
    if (mode == "inside") {
      if (vtr_writer_flush(w))
        return 3;
      vtr_writer_enter(w);
      std::raise(SIGABRT);
    }
    if (mode == "abort")
      std::raise(SIGABRT);
    if (mode == "deaf" || mode == "stop-inside") {
      if (mode == "stop-inside") {
        if (vtr_writer_flush(w))
          return 3;
        vtr_writer_enter(w);
      }
      std::raise(SIGTERM);
      for (;;)
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
    if (mode == "stop") {
      std::raise(SIGTERM);
      for (unsigned i = 0; !vtr_guard_stop_requested() && i < 1000; ++i)
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
      vtr_ending e;
      vtr_guard_ending(&e);
      return vtr_writer_close_ending(w, &e);
    }
    return vtr_writer_close(w);
  } catch (const std::exception &e) {
    std::cerr << e.what() << '\n';
    return 1;
  }
}
