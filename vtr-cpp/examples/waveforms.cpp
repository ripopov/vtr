#include "vtr_reader.h"
#include "vtr_writer.h"

#include <iostream>

int main(int argc, char** argv) {
    namespace vt = vtr;
    try {
        auto path = argc > 1 ? argv[1] : "waves.vtr";
        vt::SignalId counter{0};
        {
            vt::Writer trace(path);
            auto core = trace.scope("core0");
            counter = trace.signal({.name = "counter", .parent = core}, {.width = 32});
            auto temperature = trace.signal({.name = "temperature", .parent = core},
                                            {.encoding = vt::SignalEncoding::real});
            auto bus = trace.signal({.name = "bus", .parent = core},
                                    {.width = 4, .alphabet = vt::States::four});
            auto alias = trace.alias(counter, {.name = "counter_alias", .parent = core});
            (void)alias;
            for (std::uint64_t tick = 0; tick < 10; ++tick) {
                trace.advance_time(tick);
                trace.emit(counter, tick);
                trace.emit_real(temperature, 20.0 + static_cast<double>(tick) * 0.25);
            }
            // Four least-significant-first two-bit states: 0, 1, X, Z.
            const vt::Bytes states{std::byte{0xe4}};
            trace.emit_packed(bus, states);
            trace.close();
        }
        vt::Reader input(path);
        auto found = input.find_signal("core0.counter");
        if (!found) {
            return 1;
        }
        std::cout << "Counter at tick 5: " << input.value_at(*found, 5).as_u64() << '\n';
        input.changes(found, {3, 6}, [](const vt::SignalChange& change) {
            std::cout << change.time << ": " << change.value.as_u64() << '\n';
            return vt::Flow::next;
        });
        auto index = vt::ActivityIndex::build(input);
        auto sidecar = index.encode();
        auto loaded = vt::ActivityIndex::load(input, sidecar);
        std::cout << loaded.active({3, 6}).size() << " active signals in [3, 6]\n";
        (void)input.validate();
    } catch (const vt::Error& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
