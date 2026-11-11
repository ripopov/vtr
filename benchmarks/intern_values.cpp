#include "vtr_reader.h"
#include "vtr_writer.h"

#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>

namespace vt = vtr;
using Clock = std::chrono::steady_clock;

std::uint32_t instruction_at(std::uint64_t ordinal) {
    return static_cast<std::uint32_t>((ordinal * 2654435761ull) % 1024);
}

vt::Bytes machine_code(std::uint32_t index) {
    vt::Bytes result;
    auto state = index + 1;
    for (unsigned i = 0; i < 32; ++i) {
        state = state * 1664525u + 1013904223u;
        result.push_back(std::byte(state >> 24));
    }
    return result;
}

std::uint64_t peak_rss_kib() {
    std::ifstream status("/proc/self/status");
    std::string line;
    while (std::getline(status, line)) {
        if (line.rfind("VmHWM:", 0) == 0)
            return std::stoull(line.substr(6));
    }
    throw std::runtime_error("cannot read process peak RSS");
}

int main(int argc, char** argv) {
    try {
        if (argc != 6)
            throw std::runtime_error("usage: intern_values PATH inline|interned "
                                     "lz4|zstd COUNT write|read");
        const std::filesystem::path path(argv[1]);
        const auto representation = std::string_view(argv[2]);
        const auto compression = std::string_view(argv[3]);
        const auto operation = std::string_view(argv[5]);
        if ((representation != "inline" && representation != "interned") ||
            (compression != "lz4" && compression != "zstd") ||
            (operation != "write" && operation != "read"))
            throw std::runtime_error("invalid benchmark configuration");
        const bool interned = representation == "interned";
        const auto codec = compression == "lz4" ? vt::Codec::lz4 : vt::Codec::zstd;
        const auto count = std::stoull(argv[4]);
        const auto start = Clock::now();
        std::uint64_t checksum = 0;
        if (operation == "write") {
            vt::WriterOptions options;
            options.mode = vt::ExecutionMode::inline_;
            options.encoding.compression = codec;
            options.encoding.block_records = 8192;
            vt::Writer writer(path, options);
            auto pc = writer.intern("ip"), code = writer.intern("machine_code");
            auto mnemonic = writer.intern("mnemonic"), pipe = writer.intern("pipe");
            auto description = writer.intern("instruction"), mask = writer.intern("exec_mask");
            std::vector<vt::Value> descriptions;
            std::vector<vt::ValueId> ids;
            for (unsigned i = 0; i < 1024; ++i) {
                descriptions.emplace_back(vt::Value::Map{
                    {pc, vt::Value(vt::Address{0x1000 + i * 4ull})},
                    {code, vt::Value(machine_code(i))},
                    {mnemonic,
                     vt::Value(std::string(i % 2 ? "FMUL R1, R2, R3" : "FADD R4, R5, R6"))},
                    {pipe, vt::Value(std::string("FP32"))}});
                if (interned)
                    ids.push_back(writer.intern_value(descriptions.back()));
            }
            auto source = writer.transactions({.name = "instructions"}, "INSTRUCTION");
            for (std::uint64_t i = 0; i < count; ++i) {
                writer.advance_time(i);
                const auto index = instruction_at(i);
                auto tx = writer.begin(source);
                writer.attribute(tx, description,
                                 interned ? vt::Value(ids[index]) : descriptions[index]);
                writer.attribute(tx, mask, std::uint64_t(1) << (i % 32));
                writer.end(tx, vt::Status::ok);
            }
            writer.close();
        } else {
            vt::Reader reader(path);
            std::uint64_t ordinal = 0;
            reader.transactions({}, [&](const vt::Record& tx) {
                const auto index = instruction_at(ordinal);
                if (tx.begin() != ordinal || tx.end() != ordinal)
                    throw std::runtime_error("transaction time changed");
                const auto attrs = tx.attributes();
                if (attrs.size() != 2 || reader.string(attrs[0].key) != "instruction" ||
                    reader.string(attrs[1].key) != "exec_mask")
                    throw std::runtime_error("attributes changed");
                if (std::holds_alternative<vt::ValueId>(attrs[0].data.data) != interned)
                    throw std::runtime_error("capture representation changed");
                const auto& fields =
                    std::get<vt::Value::Map>(reader.resolve_value(attrs[0].data).data);
                if (fields.size() != 4 || reader.string(fields[0].key) != "ip" ||
                    reader.string(fields[1].key) != "machine_code" ||
                    reader.string(fields[2].key) != "mnemonic" ||
                    reader.string(fields[3].key) != "pipe" ||
                    std::get<vt::Address>(fields[0].data.data).value != 0x1000 + index * 4ull ||
                    std::get<vt::Bytes>(fields[1].data.data) != machine_code(index) ||
                    std::get<std::string>(fields[2].data.data) !=
                        (index % 2 ? "FMUL R1, R2, R3" : "FADD R4, R5, R6") ||
                    std::get<std::string>(fields[3].data.data) != "FP32" ||
                    std::get<std::uint64_t>(attrs[1].data.data) !=
                        (std::uint64_t(1) << (ordinal % 32)))
                    throw std::runtime_error("instruction content changed");
                checksum += index + ordinal;
                ++ordinal;
                return vt::Flow::next;
            });
            if (ordinal != count)
                throw std::runtime_error("transaction count changed");
        }
        const auto seconds = std::chrono::duration<double>(Clock::now() - start).count();
        const auto rss = peak_rss_kib();
        std::cout << "{\"seconds\":" << seconds
                  << ",\"file_bytes\":" << std::filesystem::file_size(path)
                  << ",\"peak_rss_kib\":" << rss << ",\"checksum\":" << checksum << "}\n";
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
