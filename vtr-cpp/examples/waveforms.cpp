#include "vtr_reader.h"
#include "vtr_writer.h"

#include <array>
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <iostream>

namespace {

namespace vt = vtr;

void write_waveforms(const std::filesystem::path& path) {
    vt::Writer trace(path);
    const auto core = trace.scope("core0");
    const auto counter = trace.signal({.name = "counter", .parent = core}, {.width = 32});
    const auto temperature = trace.signal({.name = "temperature", .parent = core},
                                          {.encoding = vt::SignalEncoding::real});
    const auto bus =
        trace.signal({.name = "bus", .parent = core}, {.width = 4, .alphabet = vt::States::four});
    (void)trace.alias(counter, {.name = "counter_alias", .parent = core});

    // Rotate 0, 1, X, Z across four least-significant-first two-bit states.
    const std::array packed_bus_states{
        std::byte{0xe4}, std::byte{0x39}, std::byte{0x4e}, std::byte{0x93}};
    for (std::uint64_t tick = 0; tick < 10; ++tick) {
        trace.advance_time(tick);
        trace.emit(counter, tick);
        trace.emit_real(temperature, 20.0 + static_cast<double>(tick) * 0.25);
        const vt::Bytes bus_value{packed_bus_states[tick % packed_bus_states.size()]};
        trace.emit_packed(bus, bus_value);
    }

    trace.close();
}

int read_waveforms(const std::filesystem::path& path) {
    vt::Reader input(path);
    const auto counter = input.find_signal("core0.counter");
    if (!counter) {
        std::cerr << "No core0.counter signal in this file\n";
        return 1;
    }

    std::cout << "Counter at tick 5: " << input.value_at(*counter, 5).as_u64() << '\n';
    input.changes(counter, {3, 6}, [](const vt::SignalChange& change) {
        std::cout << change.time << ": " << change.value.as_u64() << '\n';
        return vt::Flow::next;
    });

    const auto activity_index = vt::ActivityIndex::build(input);
    const auto encoded_index = activity_index.encode();
    const auto loaded_index = vt::ActivityIndex::load(input, encoded_index);
    std::cout << loaded_index.active({3, 6}).size() << " active signals in [3, 6]\n";
    (void)input.validate();
    return 0;
}

} // namespace

int main(int argc, char** argv) {
    try {
        const std::filesystem::path path = argc > 1 ? argv[1] : "waves.vtr";
        write_waveforms(path);
        return read_waveforms(path);
    } catch (const vt::Error& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
