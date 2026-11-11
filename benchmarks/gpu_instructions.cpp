#include "vtr_reader.h"
#include "vtr_writer.h"
#include "workload.hpp"

#include <chrono>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <sys/resource.h>

namespace vt = vtr;
using bench::u64;
using Clock = std::chrono::steady_clock;

namespace {
constexpr unsigned waves = 128, descriptions = 50;
constexpr u64 default_count = 262144;
constexpr std::array<std::string_view, 5> phase_names{
    "buffered", "scoreboard", "ready", "execute", "writeback"};
constexpr std::array<std::string_view, 9> description_names{
    "pc", "machine_code", "mnemonic", "opcode_class", "execution", "atomic_result",
    "size_bytes", "gfx", "execution_pipe"};

struct Dynamic {
    u64 sequence;
    vt::StringId label{0};
    bool issued;
    u64 exec_mask;
    std::uint8_t segment;
};

struct Instruction {
    u64 sequence;
    vt::Address pc;
    vt::StringId label{0};
    std::uint8_t opcode, execution, atomic, size;
    bool issued;
    u64 exec_mask;
    std::uint8_t segment;
    u64 machine_code;
    std::uint32_t gfx;
    vt::StringId pipe{0};
};

} // namespace

template <> struct vt::Schema<Dynamic> {
    static constexpr std::string_view name = "gpu.DynamicInstruction";
    static constexpr auto fields = std::tuple{
        vt::field("sequence", &Dynamic::sequence), vt::field("vtr.label", &Dynamic::label),
        vt::field("issued", &Dynamic::issued), vt::field("exec_mask", &Dynamic::exec_mask),
        vt::field("resolved_segment", &Dynamic::segment)};
};

template <> struct vt::Schema<Instruction> {
    static constexpr std::string_view name = "gpu.Instruction";
    static constexpr auto fields = std::tuple{
        vt::field("sequence", &Instruction::sequence), vt::field("pc", &Instruction::pc),
        vt::field("vtr.label", &Instruction::label), vt::field("opcode_class", &Instruction::opcode),
        vt::field("execution", &Instruction::execution), vt::field("atomic_result", &Instruction::atomic),
        vt::field("size_bytes", &Instruction::size), vt::field("issued", &Instruction::issued),
        vt::field("exec_mask", &Instruction::exec_mask), vt::field("resolved_segment", &Instruction::segment),
        vt::field("machine_code", &Instruction::machine_code), vt::field("gfx", &Instruction::gfx),
        vt::field("execution_pipe", &Instruction::pipe)};
};

namespace {

void require(bool ok, const char* message) {
    if (!ok) throw std::runtime_error(message);
}

double seconds(Clock::time_point start) {
    return std::chrono::duration<double>(Clock::now() - start).count();
}

u64 peak_rss_kib() {
    std::ifstream input("/proc/self/status");
    std::string line;
    while (std::getline(input, line))
        if (line.starts_with("VmHWM:")) return std::stoull(line.substr(6));
    throw std::runtime_error("cannot read process peak RSS");
}

unsigned instruction_at(u64 i) {
    return unsigned((i / waves + (i % waves) % 4 * 7) % descriptions);
}

bool memory(unsigned index) { return index % 5 < 2; }
u64 logical_bytes(unsigned index) {
    constexpr u64 record = 8 + 4 + 8 + 8 + 8 + 1;
    constexpr u64 payload = 8 + 8 + 4 + 4 + 1 + 8 + 1 + 8 + 4 + 4;
    constexpr u64 phase = 8 + 4 + 4 + 8 + 8;
    constexpr u64 relation = 4 + 8 + 8;
    constexpr u64 phase_attribute = 4 + 8 + 4;
    return record + payload + 5 * phase + relation + (memory(index) ? phase_attribute : 0);
}
u64 mask(u64 i) { return i % 31 ? UINT64_MAX : 0xffffffff; }
u64 begin(u64 i) { return 16 * i + 16; }
u64 pc(unsigned index) { return 0x100000 + index * 8; }
u64 machine_code(unsigned index) { return 0xdc50000000000000ULL + index; }
std::string label(unsigned index) {
    return (memory(index) ? "global_load_dword v" : "v_add_u32 v") +
           std::to_string(index) + ", v[0:1]";
}

bench::Digest oracle(u64 count) {
    bench::Digest result;
    for (u64 i = 0; i < count; ++i) {
        auto index = instruction_at(i);
        result.add(bench::mix(i ^ pc(index) ^ mask(i)), logical_bytes(index));
    }
    return result;
}

void print_digest(const bench::Digest& d) {
    std::cout << "{\"count\":" << d.count << ",\"sum\":\"" << d.sum << "\",\"xor\":\"" << d.xors
              << "\",\"logical_bytes\":" << d.logical_bytes << '}';
}

struct WriteResult {
    double capture = 0, close = 0;
    vt::WriterStats stats;
};

template <class Payload>
WriteResult write(const std::string& path, u64 count, const vt::WriterOptions& options) {
    constexpr bool interned = std::same_as<Payload, Dynamic>;
    vt::Writer writer(path, options);
    auto root = writer.scope("gpu");
    auto owners = writer.transactions({.name = "waves", .parent = root}, "GPU_WAVE");
    std::vector<vt::Transaction<vt::Unit>> parents;
    std::vector<vt::Source<Payload, vt::SourceKind::transaction>> sources;
    for (unsigned i = 0; i < waves; ++i) {
        auto scope = writer.scope("wave" + std::to_string(i), root);
        sources.push_back(writer.transactions<Payload>(
            {.name = "instructions", .parent = scope}, "GPU_INSTRUCTION"));
        parents.push_back(writer.begin_at(owners, 0));
    }
    std::vector<vt::StringId> keys;
    for (auto name : description_names) keys.push_back(writer.intern(name));
    auto description_key = writer.intern("instruction"), ownership = writer.intern("owns");
    auto memory_key = writer.intern("memory_stage"), memory_name = writer.intern("completed");
    std::vector<vt::StringId> phases;
    for (auto name : phase_names)
        phases.push_back(writer.intern(name));
    auto scalar = writer.intern("scalar_alu"), vector = writer.intern("vector_alu");
    auto global = writer.intern("global_memory");
    std::vector<Instruction> code;
    std::vector<vt::ValueId> ids;
    for (unsigned i = 0; i < descriptions; ++i) {
        auto text = writer.intern(label(i));
        auto pipe = memory(i) ? global : i % 3 ? vector : scalar;
        code.push_back({0, {pc(i)}, text, std::uint8_t(memory(i) ? 1 : 0),
                        std::uint8_t(i % 3 != 0), 0, std::uint8_t(i % 2 ? 8 : 4),
                        true, 0, std::uint8_t(memory(i)), machine_code(i), 942, pipe});
        const auto& facts = code.back();
        if constexpr (interned) {
            ids.push_back(writer.intern_value(vt::Value::Map{
                {keys[0], facts.pc}, {keys[1], facts.machine_code}, {keys[2], text},
                {keys[3], facts.opcode}, {keys[4], facts.execution}, {keys[5], facts.atomic},
                {keys[6], facts.size}, {keys[7], facts.gfx}, {keys[8], pipe}}));
        }
    }
    auto start = Clock::now();
    for (u64 i = 0; i < count; ++i) {
        const auto index = instruction_at(i), wave = unsigned(i % waves);
        const auto tick = begin(i);
        writer.advance_time(tick);
        auto facts = code[index];
        facts.sequence = i;
        facts.exec_mask = mask(i);
        Payload payload;
        if constexpr (interned) payload = {i, facts.label, true, facts.exec_mask, facts.segment};
        else payload = facts;
        auto tx = writer.begin_at(sources[wave], tick, payload, {.parent = parents[wave]});
        if constexpr (interned) writer.attribute(tx, description_key, ids[index]);
        for (unsigned j = 0; j < phases.size(); ++j) {
            vt::Attributes attributes;
            if (memory(index) && j == 3)
                attributes.push_back({memory_key, vt::EnumValue{2, memory_name}});
            writer.embedded_stage(tx, tick + 2 * j, phases[j], {}, tick + 2 * j + 2, attributes);
        }
        writer.relate(ownership, parents[wave].id(), tx.id());
        writer.end_at(tx, tick + 12, vt::Status::ok);
    }
    for (auto parent : parents) writer.end_at(parent, begin(count) + 12, vt::Status::ok);
    WriteResult result;
    result.capture = seconds(start);
    start = Clock::now();
    writer.close();
    result.close = seconds(start);
    result.stats = writer.stats();
    return result;
}

bench::Digest read(const std::string& path, u64 count, bool interned) {
    vt::Reader reader(path, {.verify_checksums = false});
    bench::Digest result;
    u64 relations = 0, owners = 0;
    std::array<bool, waves> seen_owners{};
    std::vector<bool> seen(count), linked(count);
    reader.transactions({}, [&](const vt::Record& tx) {
        if (reader.string(reader.nodes()[tx.generator()->value].name) != "instructions") {
            require(tx.id().value && tx.id().value <= waves && !tx.parent() &&
                        tx.begin() == 0 && tx.end() == begin(count) + 12 &&
                        tx.status() == vt::Status::ok, "wave lifetime changed");
            require(!seen_owners[tx.id().value - 1], "wave duplicated");
            seen_owners[tx.id().value - 1] = true;
            ++owners;
            return vt::Flow::next;
        }
        auto dynamic = interned ? tx.payload().as<Dynamic>() : Dynamic{};
        auto full = interned ? Instruction{} : tx.payload().as<Instruction>();
        const auto i = interned ? dynamic.sequence : full.sequence;
        require(i < count && !seen[i], "instruction sequence duplicated or out of range");
        seen[i] = true;
        const auto index = instruction_at(i);
        if (interned) {
            auto attrs = tx.attributes();
            require(attrs.size() == 1 && reader.string(attrs[0].key) == "instruction" &&
                        std::holds_alternative<vt::ValueId>(attrs[0].data.data),
                    "description attribute changed");
            auto& fields = std::get<vt::Value::Map>(reader.resolve_value(attrs[0].data).data);
            require(fields.size() == 9, "description fields changed");
            for (unsigned field = 0; field < fields.size(); ++field)
                require(reader.string(fields[field].key) == description_names[field], "description key changed");
            full = {i, std::get<vt::Address>(fields[0].data.data), dynamic.label,
                    std::get<std::uint8_t>(fields[3].data.data),
                    std::get<std::uint8_t>(fields[4].data.data),
                    std::get<std::uint8_t>(fields[5].data.data),
                    std::get<std::uint8_t>(fields[6].data.data), dynamic.issued,
                    dynamic.exec_mask, dynamic.segment, std::get<u64>(fields[1].data.data),
                    std::get<std::uint32_t>(fields[7].data.data),
                    std::get<vt::StringId>(fields[8].data.data)};
            require(reader.string(std::get<vt::StringId>(fields[2].data.data)) == label(index),
                    "shared mnemonic changed");
        } else require(tx.attributes().empty(), "typed payload gained attributes");
        require(full.pc.value == pc(index) && full.machine_code == machine_code(index) &&
                    reader.string(full.label) == label(index) && full.gfx == 942 &&
                    full.opcode == memory(index) && full.execution == (index % 3 != 0) &&
                    full.atomic == 0 && full.size == (index % 2 ? 8 : 4) && full.issued &&
                    full.exec_mask == mask(i) && full.segment == memory(index) &&
                    reader.string(full.pipe) == (memory(index) ? "global_memory" :
                                                index % 3 ? "vector_alu" : "scalar_alu"),
                "instruction content changed");
        require(tx.id().value == waves + 6 * i + 1 && tx.parent() && tx.parent()->value == i % waves + 1 &&
                    tx.begin() == begin(i) && tx.end() == begin(i) + 12 &&
                    tx.status() == vt::Status::ok && tx.stages().size() == 5,
                "instruction identity, ownership, timing or phases changed");
        for (unsigned j = 0; j < 5; ++j) {
            const auto& phase = tx.stages()[j];
            require(phase.id.value == tx.id().value + j + 1 &&
                        phase.begin == begin(i) + 2 * j && phase.end == begin(i) + 2 * j + 2 &&
                        reader.string(phase.name) == phase_names[j] && reader.string(phase.lane).empty(),
                    "embedded phase identity, lane, name or timing changed");
            require(phase.data.size() == (memory(index) && j == 3 ? 1 : 0),
                    "memory phase attributes changed");
            if (!phase.data.empty()) {
                const auto& value = std::get<vt::EnumValue>(phase.data[0].data.data);
                require(reader.string(phase.data[0].key) == "memory_stage" && value.value == 2 &&
                            reader.string(value.literal) == "completed", "memory phase status changed");
            }
        }
        result.add(bench::mix(i ^ full.pc.value ^ full.exec_mask), logical_bytes(index));
        return vt::Flow::next;
    });
    for (unsigned wave = 0; wave < waves; ++wave) {
        reader.relations(vt::RecordId{wave + 1}, vt::Direction::outgoing,
                         [&](const vt::Relation& relation) {
            require(reader.string(relation.kind) == "owns" && relation.from == vt::RecordId{wave + 1} &&
                        relation.to && relation.to->value > waves &&
                        (relation.to->value - waves - 1) % 6 == 0 && relation.data.empty() &&
                        ((relation.to->value - waves - 1) / 6) % waves == wave,
                    "wave ownership relation changed");
            const auto i = (relation.to->value - waves - 1) / 6;
            require(i < count && !linked[i], "ownership relation duplicated or out of range");
            linked[i] = true;
            ++relations;
            return vt::Flow::next;
        });
    }
    require(result.count == count && relations == count && owners == waves,
            "instruction, wave or relation count changed");
    return result;
}

} // namespace

int gpu_benchmark(int argc, char** argv) {
    try {
        require(argc == 6, "usage: gpu_instructions OP FORMAT gpu-instructions PATH SCALE_DIVISOR");
        std::string op = argv[1], format = argv[2], profile = argv[3], path = argv[4];
        if (format == "vtr") format = "vtr-interned";
        require(profile == "gpu-instructions" || profile == "gpu-instructions-zstd", "unknown GPU profile");
        require(format == "null" || format == "vtr-payload" || format == "vtr-interned" ||
                    format == "vtr-payload-fixed" || format == "vtr-interned-fixed" ||
                    format == "vtr-payload-batched" || format == "vtr-interned-batched", "unknown GPU format");
        auto scale = std::stoul(argv[5]);
        require(scale && scale <= 10000, "scale divisor must be 1..10000");
        const auto count = std::max<u64>(64, default_count / scale);
        std::cout << std::setprecision(12);
        if (op == "oracle") {
            std::cout << "{\"all\":";
            print_digest(oracle(count));
            std::cout << "}\n";
            return 0;
        }
        const auto start = Clock::now();
        WriteResult written;
        bench::Digest decoded;
        const bool interned = format.find("interned") != std::string::npos;
        vt::WriterOptions options;
        options.mode = vt::ExecutionMode::inline_;
        options.encoding.compression = profile.ends_with("-zstd") ? vt::Codec::zstd : vt::Codec::lz4;
        if (format.ends_with("-fixed")) {
            options.encoding.block_bytes = 64 << 20;
            options.encoding.block_records = 2048;
        } else if (format.ends_with("-batched")) options.encoding.block_bytes = 2 << 20;
        u64 baseline_count = 0, baseline_checksum = 0;
        if (op == "write") {
            if (format == "null") {
                const auto generated = oracle(count);
                baseline_count = generated.count;
                baseline_checksum = generated.sum;
                written.capture = seconds(start);
            } else if (interned) written = write<Dynamic>(path, count, options);
            else written = write<Instruction>(path, count, options);
        } else if (op == "read") decoded = read(path, count, interned);
        else throw std::runtime_error("unknown GPU benchmark operation");
        auto elapsed = seconds(start);
        rusage usage{};
        require(!getrusage(RUSAGE_SELF, &usage), "getrusage failed");
        const double cpu = usage.ru_utime.tv_sec + usage.ru_utime.tv_usec * 1e-6 +
                           usage.ru_stime.tv_sec + usage.ru_stime.tv_usec * 1e-6;
        std::cout << "{\"seconds\":" << elapsed << ",\"capture_seconds\":" << written.capture
                  << ",\"close_seconds\":" << written.close << ",\"cpu_seconds\":" << cpu
                  << ",\"peak_rss_kib\":" << peak_rss_kib() << ",\"ticks\":" << count
                  << ",\"baseline_count\":" << baseline_count
                  << ",\"baseline_checksum\":\"" << baseline_checksum << '"'
                  << ",\"transaction_blocks\":" << written.stats.transaction_blocks
                  << ",\"peak_transaction_bytes\":" << written.stats.peak_transaction_bytes
                  << ",\"digests\":[";
        if (op == "read") print_digest(decoded);
        std::cout << "],\"latencies_seconds\":[]}\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "gpu benchmark: " << error.what() << '\n';
        return 1;
    }
}
