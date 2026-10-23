#include "vtr_reader.h"
#include "vtr_trace_dpi.h"

#include <climits>
#include <filesystem>
#include <iostream>
#include <thread>

const char* g_test_scope = "TOP.core.instrumentation";

namespace {
void check(bool value, const char* message) {
    if (!value) throw std::runtime_error(message);
}

struct Host final : vtr_trace::Host {
    std::uint64_t time = 0;
    std::uint64_t now() override { return time; }
    int timePrecision() override { return -12; }
    std::string rootPath() override { return "TOP"; }
};

struct Sink final : vtr_trace::Sink {
    vtr::Writer& output;
    std::optional<vtr::ProducerId> producer;
    std::uint64_t time = 0;
    unsigned depth = 0;
    std::map<std::string, vtr::ScopeId> scopes;
    std::vector<std::string> warnings;
    Sink(vtr::Writer& writer, vtr::ExecutionMode mode) : output(writer) {
        if (mode == vtr::ExecutionMode::threaded) producer = writer.add_producer();
    }
    vtr::Writer& writer() override { return output; }
    void enter(std::uint64_t next) override {
        if (depth) { ++depth; return; }
        if (producer) output.use_producer(*producer);
        try {
            if (next > time) output.advance_time(next);
            time = std::max(time, next);
            depth = 1;
        } catch (...) {
            if (producer) output.release_producer();
            throw;
        }
    }
    void leave() noexcept override {
        if (!--depth && producer) output.release_producer();
    }
    int timescale() override { return -12; }
    std::optional<vtr::ScopeId> scopeNode(const std::string& path) override {
        if (path.empty()) return {};
        if (auto found = scopes.find(path); found != scopes.end()) return found->second;
        const auto dot = path.rfind('.');
        const auto parent = dot == std::string::npos ? std::optional<vtr::ScopeId>{} : scopeNode(path.substr(0, dot));
        const auto node = output.scope(dot == std::string::npos ? path : path.substr(dot + 1), parent);
        scopes.emplace(path, node);
        return node;
    }
    void warn(const std::string& message) override { warnings.push_back(message); }
};

vtr_trace::Runtime* current = nullptr;

const vtr::Value& attribute(const vtr::Reader& reader, const vtr::Record& record, std::string_view name) {
    for (const auto& attr : record.attributes()) if (reader.string(attr.key) == name) return attr.data;
    throw std::runtime_error("missing attribute");
}

void run(const std::filesystem::path& path, vtr::ExecutionMode mode) {
    Host host;
    vtr_trace::Runtime runtime(host);
    current = &runtime;
    const auto clock = vtr_clock("/TOP.core", "clk");
    const auto pipeline = vtr_pipeline("/TOP.core", "instructions", "clk");
    const auto lsu = vtr_tracker("/TOP.core", "lsu", "LSU", "");
    (void)vtr_tracker("/TOP.core", "unused", "TRANSACTION", "absent");
    const auto fetched = vtr_keyspace(pipeline, "fetch"), issued = vtr_keyspace(pipeline, "issue");
    const auto queue = vtr_keyspace(lsu, "queue");
    vtr_clock_run(clock, 4, -12);
    vtr_item_open(fetched, 999, "ignored before recording");
    host.time = 4;
    {
        vtr::Writer writer(path, {.mode = mode});
        Sink sink(writer, mode);
        runtime.opened(sink);
        bool rejected = false;
        try { runtime.opened(sink); } catch (const vtr::Error&) { rejected = true; }
        check(rejected, "second live sink rejected");
        vtr_item_open(fetched, 7, "IF");
        vtr_item_attr_u64(fetched, 7, "pc", 0x100);
        vtr_item_label(fetched, 7, "one");
        vtr_item_bind(fetched, 7, issued, 3);
        const auto saved = vtr_now();
        host.time = 5;
        const auto advance = [&] {
            vtr_item_stage(issued, 3, "EX");
            vtr_item_event(issued, 3, "replay");
            vtr_item_event(issued, 3, "replay");
        };
        if (mode == vtr::ExecutionMode::threaded) {
            std::exception_ptr failure;
            std::thread worker([&] { try { advance(); } catch (...) { failure = std::current_exception(); } });
            worker.join();
            if (failure) std::rethrow_exception(failure);
        } else advance();
        vtr_item_lane(issued, 3, "memory", "DC");
        vtr_item_lane(issued, 3, "memory", "");
        vtr_item_open_child(queue, 42, "Q", issued, 3);
        vtr_item_open_child_at(queue, 43, "Q", issued, 3, saved);
        vtr_item_open_child(queue, 99, "Q", issued, 1234);
        host.time = 6;
        vtr_item_open_at(fetched, 8, "IF", saved);
        vtr_item_stage_at(fetched, 8, "HIST", 5);
        std::string text = "copied";
        vtr_item_attr_str(fetched, 8, "text", text.c_str());
        text.assign("mutated");
        host.time = 7;
        check(vtr_item_bind_oldest(fetched, 2, issued, 9) == 2, "DPI fused count");
        vtr_item_attr_u64(issued, 9, "pc", 0x200);
        vtr_item_event(issued, 9, "group");
        host.time = 8;
        vtr_item_relate("loads", issued, 9, queue, 42);
        vtr_item_close(queue, 42, 1);
        vtr_item_close(queue, 43, 1);
        vtr_item_close(issued, 9, 1);
        host.time = 9;
        vtr_clock_stop(clock);
        vtr_clock_run(clock, 1, INT_MIN);
        vtr_clock_stop(clock);
        vtr_item_stage(pipeline, 1, "bad handle");
        vtr_item_close(issued, 9, 0);
        host.time = 10;
        runtime.closing();
        check(!sink.depth && sink.warnings.size() == 5, "DPI aggregated diagnostics");
        writer.close();
    }
    vtr::Reader reader(path);
    check(reader.validate().missing.empty(), "DPI output validates");
    const auto instructions = *reader.find_generator("TOP.core.instructions");
    const auto loads = *reader.find_generator("TOP.core.lsu");
    const auto clk = *reader.find_generator("TOP.core.clk");
    std::vector<vtr::Record> owners, children;
    reader.transactions({}, [&](const auto& record) {
        if (record.generator() == instructions) owners.push_back(record);
        if (record.generator() == loads) children.push_back(record);
        return vtr::Flow::next;
    });
    check(owners.size() == 2 && children.size() == 2, "DPI creates only traced owners");
    const auto& parent = owners.front();
    check(parent.begin() == 4 && parent.end() == 8 && parent.stages().size() == 3 && parent.events().size() == 3,
          "DPI stages and repeated point events");
    check(owners.back().begin() == 4 && owners.back().stages().size() == 2 && owners.back().events().size() == 1,
          "DPI historical observation times");
    check(reader.string(std::get<vtr::StringId>(attribute(reader, owners.back(), "text").data)) == "copied",
          "DPI string input copied");
    check(children.front().parent() == parent.id() && parent.id() < children.front().id() &&
              children.front().begin() == 5, "DPI parent fixed at child creation");
    check(children.back().parent() == parent.id() && parent.id() < children.back().id() &&
              children.back().begin() == 4 && children.back().end() == 8 &&
              children.back().stages().front().begin == 4,
          "DPI historical child fixes its parent at creation and retains observation time");
    for (const auto& owner : owners) {
        check(owner.status() == vtr::Status::ok && std::get<std::uint64_t>(attribute(reader, owner, "pc").data) == 0x200,
              "DPI final assignments and fused close");
        unsigned edges = 0;
        reader.relations(owner.id(), vtr::Direction::outgoing, [&](const auto& relation) {
            check(relation.to == children.front().id() && reader.string(relation.kind) == "loads", "DPI relation endpoints");
            ++edges; return vtr::Flow::next;
        });
        check(edges == 1, "DPI fused relation expansion");
    }
    bool associated = false;
    for (const auto& generator : reader.generators())
        if (generator.id == instructions) associated = generator.clock == clk;
    check(associated && reader.cycle_at(clk, 7) == 1 && reader.cycle_at(clk, 10) == 2,
          "DPI clock association and stopped edge timeline");
    (void)vtr_clock("/TOP.core", "absent");
    const auto reopened = vtr_keyspace(pipeline, "reopened");
    host.time = 20;
    {
        vtr::Writer writer(path.string() + ".reopen", {.mode = mode});
        Sink sink(writer, mode);
        runtime.opened(sink);
        vtr_item_open(reopened, 1, "NEW");
        vtr_item_close(reopened, 1, 1);
        runtime.closing();
        check(sink.warnings.empty(), "reopened declarations use new recording handles");
        writer.close();
    }
    check(vtr::Reader(path.string() + ".reopen").validate().missing.empty(), "DPI reopened file validates");
    current = nullptr;
}

void unused_package(const std::filesystem::path& path, vtr::ExecutionMode mode) {
    Host host;
    vtr_trace::Runtime runtime(host);
    vtr::Writer writer(path, {.mode = mode});
    const auto signal = writer.signal({.name = "clock"});
    Sink sink(writer, mode);
    runtime.opened(sink);
    sink.enter(0);
    writer.emit(signal, 0);
    writer.advance_time(7);
    sink.leave();
    host.time = 9;
    runtime.closing();
    writer.close();
    vtr::Reader reader(path);
    const auto range = reader.waveform_time_range();
    check(reader.validate().missing.empty() && range && range->first == 0 && range->last == 7,
          "unused SDK preserves last explicit waveform timestamp, including empty timestamps");
}
}

vtr_trace::Runtime& vtr_trace::runtime() { return *current; }
void vtr_trace::failure(const char* message) { throw std::runtime_error(message); }

int main(int argc, char** argv) {
    try {
        const std::filesystem::path root(argc > 1 ? argv[1] : "dpi-fixtures");
        std::filesystem::create_directories(root);
        for (auto mode : {vtr::ExecutionMode::inline_, vtr::ExecutionMode::threaded}) {
            run(root / (mode == vtr::ExecutionMode::inline_ ? "inline.vtr" : "threaded.vtr"), mode);
            unused_package(root / (mode == vtr::ExecutionMode::inline_ ? "unused-inline.vtr" : "unused-threaded.vtr"), mode);
        }
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
