#include "vtr_reader.h"
#include "vtr_track.h"

#include <cstdlib>
#include <filesystem>
#include <iostream>
#include <new>
#include <thread>

thread_local bool count_allocations = false;
thread_local std::size_t allocation_calls = 0;

void* operator new(std::size_t size) {
    if (void* memory = std::malloc(size ? size : 1)) {
        if (count_allocations)
            ++allocation_calls;
        return memory;
    }
    throw std::bad_alloc();
}

void* operator new[](std::size_t size) {
    return ::operator new(size);
}

void operator delete(void* memory) noexcept {
    std::free(memory);
}

void operator delete[](void* memory) noexcept {
    std::free(memory);
}

void operator delete(void* memory, std::size_t) noexcept {
    std::free(memory);
}

void operator delete[](void* memory, std::size_t) noexcept {
    std::free(memory);
}

void* operator new(std::size_t size, std::align_val_t alignment) {
    void* memory = nullptr;
    if (posix_memalign(&memory, static_cast<std::size_t>(alignment), size ? size : 1))
        throw std::bad_alloc();
    if (count_allocations)
        ++allocation_calls;
    return memory;
}

void* operator new[](std::size_t size, std::align_val_t alignment) {
    return ::operator new(size, alignment);
}

void operator delete(void* memory, std::align_val_t) noexcept {
    std::free(memory);
}

void operator delete[](void* memory, std::align_val_t) noexcept {
    std::free(memory);
}

void operator delete(void* memory, std::size_t, std::align_val_t) noexcept {
    std::free(memory);
}

void operator delete[](void* memory, std::size_t, std::align_val_t) noexcept {
    std::free(memory);
}

namespace vt = vtr;

void check(bool condition, const char* message) {
    if (!condition)
        throw std::runtime_error(message);
}

const vt::Value& attribute(const vt::Reader& reader, const vt::Record& record,
                           std::string_view name) {
    for (const auto& item : record.attributes())
        if (reader.string(item.key) == name)
            return item.data;
    throw std::runtime_error("missing attribute");
}

void check_lifecycle(const std::filesystem::path& path, vt::RecordId first, vt::RecordId second,
                     vt::RecordId third, vt::RecordId wakeup, vt::RecordId child) {
    vt::Reader reader(path);
    check(reader.validate().missing.empty(), "full tracker validation");
    unsigned owners = 0, okay = 0, aborted = 0, open = 0;
    reader.transactions({}, [&](const auto& record) {
        ++owners;
        okay += record.status() == vt::Status::ok;
        aborted += record.status() == vt::Status::aborted;
        open += record.status() == vt::Status::open;
        return vt::Flow::next;
    });
    check(owners == 12 && okay == 3 && aborted == 8 && open == 1, "lifecycle status counts");
    for (auto id : {first, second}) {
        const auto record = reader.lookup(id);
        check(record && record->begin() == 10 && record->end() == 15,
              "fused identity and lifetime");
        check(std::get<std::uint64_t>(attribute(reader, *record, "pc").data) == 0x1000,
              "last attribute assignment wins without duplicate attributes");
        check(reader.string(std::get<vt::StringId>(attribute(reader, *record, "vtr.label").data)) ==
                  "fused",
              "last label assignment wins");
        check(record->events().size() == 2 && record->events()[0].id != record->events()[1].id,
              "repeated point events preserve distinct creation IDs");
    }
    const auto tail = reader.lookup(third);
    check(tail && tail->status() == vt::Status::open && tail->end() == 20 &&
              tail->stages().size() == 5,
          "detached record retains unfinished stage information");
    bool mem_lane = false, execute = false;
    for (const auto& stage : tail->stages()) {
        if (reader.string(stage.lane) == "mem")
            mem_lane = stage.begin == 13 && stage.end == 14;
        if (reader.string(stage.name) == "EX")
            execute = stage.begin == 13 && !stage.end;
    }
    check(mem_lane && execute, "independent lane and repeated-stage suppression");
    check(std::get<std::uint64_t>(attribute(reader, *tail, "pc").data) == 0x2000,
          "detach publishes pending attributes on unfinished owners");
    check(reader.lookup(child)->parent() == third && third < child,
          "parent fixed at child creation");
    bool linked = false;
    reader.relations(third, vt::Direction::outgoing, [&](const auto& relation) {
        linked = relation.to == wakeup && reader.string(relation.kind) == "wakeup";
        return vt::Flow::next;
    });
    check(linked && reader.lookup(wakeup)->status() == vt::Status::aborted,
          "relation survives key reuse");
}

void lifecycle(const std::filesystem::path& path, vt::ExecutionMode mode) {
    vt::Writer writer(path, {.mode = mode});
    const auto scope = writer.scope("cpu");
    const auto pipeline = writer.transactions({.name = "pipeline", .parent = scope}, "PIPELINE");
    const auto requests = writer.transactions({.name = "lsu", .parent = scope}, "LSU");
    vt::Tracker p(writer, pipeline, "cpu.pipeline"), l(writer, requests, "cpu.lsu");
    const auto fe = p.keyspace("fe"), iid = p.keyspace("iid"), reg = p.keyspace("reg");
    const auto sq = l.keyspace("sq");
    writer.advance_time(10);
    const auto first = *p.open(fe, 1, "ID", 10);
    p.attribute(fe, 1, "pc", std::uint64_t{0x100});
    p.label(fe, 1, "one");
    const auto second = *p.open(fe, 2, "ID", 10);
    const auto third = *p.open(fe, 3, "ID", 10);
    writer.advance_time(11);
    for (unsigned key = 1; key <= 3; ++key)
        p.stage(fe, key, "IR", 11);
    writer.advance_time(12);
    check(p.bind_oldest(fe, 2, iid, 5, 12) == 2, "fusion count");
    p.stage(iid, 5, "IQ", 12);
    check(p.bind_oldest(fe, 1, iid, 6, 12) == 1, "oldest count");
    p.stage(iid, 6, "IQ", 12);
    p.bind(iid, 6, reg, 40, 12);
    writer.advance_time(13);
    p.stage(reg, 40, "EX", 13);
    p.lane(reg, 40, "mem", "DC", 13);
    (void)p.open(fe, 4, "ID", 13);
    (void)p.open(fe, 5, "ID", 13);
    writer.advance_time(14);
    p.lane(reg, 40, "mem", "", 14);
    p.stage(iid, 6, "EX", 14);
    p.attribute(iid, 5, "pc", std::uint64_t{0x1000});
    p.attribute(iid, 6, "pc", std::uint64_t{0x2000});
    p.label(iid, 5, "fused");
    p.event(iid, 5, "replay", 14);
    p.event(iid, 5, "replay", 14);
    writer.advance_time(15);
    check(p.close(iid, 5, vt::Status::ok, 15) == 2, "close all fused owners");
    check(p.abort_younger(iid, 6, 15) == 2, "abort younger creation order");
    writer.advance_time(16);
    p.stage(fe, 4, "IR", 16);
    const auto wakeup = *p.open(fe, 7, "ID", 16);
    const auto child = *l.open(sq, 0, "Q", 16, p.owner(iid, 6));
    p.relate("wakeup", iid, 6, p, fe, 7);
    (void)p.open(fe, 8, "ID", 16);
    p.bind(fe, 8, iid, 9, 16);
    (void)p.open(fe, 9, "ID", 16);
    writer.advance_time(17);
    (void)p.open(fe, 7, "ID", 17);
    p.bind(fe, 9, iid, 9, 17);
    writer.advance_time(18);
    check(p.abort_keyspace(fe, 18) == 2, "abort aliases once");
    writer.advance_time(19);
    check(l.close(sq, 0, vt::Status::ok, 19) == 1, "child close");
    (void)l.open(sq, 1, "Q", 19);
    (void)l.open(sq, 2, "Q", 19);
    check(l.abort_all(19) == 2, "abort all");
    check(p.open_count() == 1 && !l.open_count(), "live tracking population");
    writer.advance_time(20);
    p.detach();
    l.detach();
    check(!p.open_count() && p.take_diagnostics().size() == 3, "misuse aggregation and detach");
    check(p.diagnostics().empty(), "take diagnostics clears counts");
    writer.close();
    check_lifecycle(path, first, second, third, wakeup, child);
}

void populations(const std::filesystem::path& path, vt::ExecutionMode mode) {
    vt::Writer writer(path, {.mode = mode});
    vt::Tracker tracker(writer, writer.transactions({.name = "requests"}), "requests");
    const auto source = tracker.keyspace("source"), fused = tracker.keyspace("fused");
    for (unsigned round = 0; round < 4; ++round) {
        const auto time = round * 4;
        writer.advance_time(time);
        for (unsigned key = 0; key < 1000; ++key)
            (void)tracker.open(source, key, "A", time);
        writer.advance_time(time + 1);
        check(tracker.bind_oldest(source, 6, fused, 1, time + 1) == 6, "overflow group fusion");
        tracker.stage(fused, 1, "X", time + 1);
        tracker.attribute(fused, 1, "folded", std::uint64_t{6});
        writer.advance_time(time + 2);
        check(tracker.close(fused, 1, vt::Status::ok, time + 2) == 6, "overflow group retirement");
        writer.advance_time(time + 3);
        for (unsigned i = 0; i < 994; ++i)
            check(tracker.close(source, 6 + (i * 7919) % 994, vt::Status::ok, time + 3) == 1,
                  "hash-table deletion preserves other keys");
        check(!tracker.open_count() && tracker.diagnostics().empty(), "reuse high-water storage");
    }
    writer.close();
    vt::Reader reader(path);
    check(reader.validate().missing.empty(), "large tracker readback");
    std::size_t count = 0;
    reader.transactions({}, [&](const auto& record) {
        ++count;
        check(record.status() == vt::Status::ok, "all shuffled completions retained");
        return vt::Flow::next;
    });
    check(count == 4000, "population identity count");
}

void handle_isolation(const std::filesystem::path& path, vt::ExecutionMode mode) {
    vt::Writer writer(path, {.mode = mode});
    vt::Tracker tracker(writer, writer.transactions({.name = "replacement"}), "replacement");
    vt::Tracker other(writer, writer.transactions({.name = "other"}), "other");
    const auto space = tracker.keyspace("keys"), foreign = other.keyspace("keys");
    writer.advance_time(1);
    const auto parent = *tracker.open(space, 1, "A", 1);
    check(!tracker.open(foreign, 1, "A", 1), "foreign keyspace rejected");
    tracker.bind(space, 1, foreign, 1, 1);
    tracker.bind(foreign, 1, space, 1, 1);
    check(!tracker.owner(foreign, 1) && tracker.records(foreign, 1).empty(),
          "foreign keyspaces never alias numeric indexes");
    writer.advance_time(2);
    const auto child = *tracker.open(space, 1, "A", 2, tracker.owner(space, 1));
    const auto live = tracker.records(space, 1);
    check(live.size() == 1 && live.front() == child, "reused key names only replacement");
    const auto external_path = path.string() + ".external";
    vt::Writer external(external_path, {.mode = mode});
    vt::Tracker destination(external, external.transactions({.name = "external"}), "external");
    const auto target = destination.keyspace("keys");
    bool rejected = false;
    try {
        tracker.relate("edge", space, 1, destination, target, 1);
    } catch (const vt::Error& error) {
        rejected = error.code() == vt::ErrorCode::wrong_recording;
    }
    check(rejected, "cross-recording relations rejected");
    external.close();
    check(tracker.close(space, 1, vt::Status::ok, 2) == 1, "replacement remains usable");
    writer.close();
    vt::Reader reader(path);
    check(reader.validate().missing.empty(), "replacement parent validation");
    check(reader.lookup(parent)->status() == vt::Status::aborted &&
              reader.lookup(child)->parent() == parent && parent < child,
          "replacement preserves parent at creation before aborting prior owner");
}

void producer_migration(const std::filesystem::path& path) {
    vt::Writer writer(path);
    vt::Tracker tracker(writer, writer.transactions({.name = "migrating"}), "migrating");
    const auto keys = tracker.keyspace("keys");
    const auto producer = writer.add_producer();
    writer.use_producer(producer);
    writer.advance_time(5);
    const auto id = *tracker.open(keys, 1, "A", 5);
    writer.release_producer();
    std::exception_ptr failure;
    std::thread worker([&] {
        try {
            writer.use_producer(producer);
            writer.advance_time(6);
            tracker.stage(keys, 1, "B", 6);
            tracker.attribute(keys, 1, "worker", std::uint64_t{1});
            (void)tracker.close(keys, 1, vt::Status::ok, 6);
            writer.finish_producer();
        } catch (...) {
            failure = std::current_exception();
        }
    });
    worker.join();
    if (failure)
        std::rethrow_exception(failure);
    writer.close();
    vt::Reader reader(path);
    check(reader.validate().missing.empty(), "migrating tracker validation");
    const auto record = reader.lookup(id);
    check(record->stages().size() == 2 && record->stages()[0].end == 6 && record->end() == 6 &&
              record->status() == vt::Status::ok,
          "producer migration preserves tracker stages and identity");
    check(std::get<std::uint64_t>(attribute(reader, *record, "worker").data) == 1,
          "migrated producer publishes attributes");
}

void steady_capture(const std::filesystem::path& path) {
    vt::Writer writer(path);
    vt::Tracker tracker(writer, writer.transactions({.name = "steady"}), "steady");
    const auto source = tracker.keyspace("source"), fused = tracker.keyspace("fused");
    const auto words95 = writer.signal({.name = "words95"}, {.width = 95});
    const auto words257 = writer.signal({.name = "words257"}, {.width = 257});
    const auto words2049 =
        writer.signal({.name = "words2049"}, {.width = 2049, .alphabet = vt::States::four});
    std::array<std::uint32_t, 65> words;
    std::size_t calls = 0;
    for (unsigned round = 0; round < 5; ++round) {
        allocation_calls = 0;
        count_allocations = round > 1;
        try {
            const auto time = round * 3;
            writer.advance_time(time);
            words.fill(UINT32_MAX);
            writer.emit_words(words95, std::span(words).first(3), 95);
            writer.emit_words(words257, std::span(words).first(9), 257);
            writer.emit_words(words2049, words, 2049);
            words.fill(0);
            for (unsigned i = 0; i < 8; ++i)
                (void)tracker.open(source, i, "A", time);
            (void)tracker.bind_oldest(source, 8, fused, 7, time);
            writer.advance_time(time + 1);
            tracker.stage(fused, 7, "X", time + 1);
            tracker.label(fused, 7, "label");
            tracker.attribute(fused, 7, "pc", std::uint64_t{123});
            tracker.event(fused, 7, "event", time + 1);
            tracker.event(fused, 7, "event", time + 1);
            tracker.relate("edge", fused, 7, tracker, fused, 7);
            writer.advance_time(time + 2);
            (void)tracker.close(fused, 7, vt::Status::ok, time + 2);
        } catch (...) {
            count_allocations = false;
            throw;
        }
        count_allocations = false;
        if (round > 1)
            calls += allocation_calls;
    }
    writer.close();
    check(vt::Reader(path).validate().missing.empty(), "allocation probe output");
    std::cout << "steady tracker producer allocations: " << calls << '\n';
    check(calls == 0, "warmed threaded tracker allocates on the producer");
    vt::Reader reader(path);
    for (auto signal : {words95, words257, words2049}) {
        const auto sample = reader.value_at(signal, 0);
        const auto stride = sample.kind.alphabet == vt::States::binary ? 1U : 2U;
        for (std::size_t bit = 0; bit < sample.kind.width; ++bit)
            check(((std::to_integer<unsigned>(sample.packed[bit * stride / 8]) >>
                    (bit * stride % 8)) &
                   ((1U << stride) - 1)) == 1,
                  "word inputs copied and converted before mutation");
    }
}

void embedded_transport(const std::filesystem::path& path, vt::ExecutionMode mode) {
    vt::Writer writer(path, {.mode = mode});
    const auto source = writer.transactions({.name = "embedded"});
    const auto name = writer.intern("visit"), lane = writer.intern("memory");
    const auto key = writer.intern("value"), text = writer.intern("nonzero string ID");
    constexpr vt::Timestamp time = 0x100000008ULL;
    writer.advance_time(time);
    const auto owner = writer.begin_at(source, time);
    writer.attribute(owner, key, vt::Value(text));
    const auto empty_event = writer.embedded_event(owner, time + 1, name);
    const auto empty_stage = writer.embedded_stage(owner, time + 2, name, lane, time + 9);
    vt::Attributes attributes{{key, vt::Value(std::string("copied input"))}};
    const auto event = writer.embedded_event(owner, time + 3, name, attributes);
    const auto stage = writer.embedded_stage(owner, time + 4, name, lane, time + 10, attributes);
    writer.relate(name, empty_event, empty_stage);
    writer.relate(name, event, stage, attributes);
    attributes.front().data = vt::Value(std::string("mutated"));
    writer.advance_time(time + 32);
    writer.end_at(owner, time + 32);
    writer.close();
    vt::Reader reader(path);
    check(reader.validate().missing.empty(), "embedded operation transport validation");
    check(std::get<vt::StringId>(attribute(reader, *reader.lookup(owner.id()), "value").data) ==
              text,
          "scalar attribute retains interned string ID");
    check(reader.lookup(empty_event)->begin() == time + 1 &&
              reader.lookup(empty_event)->attributes().empty(),
          "empty event transport");
    for (const auto id : {empty_stage, stage}) {
        const auto record = reader.lookup(id);
        check(record->lane() == lane && record->explicit_end() == time + (id == stage ? 10 : 9),
              "embedded stage fixed-width fields retain high timestamp bits");
    }
    for (const auto id : {event, stage})
        check(std::get<std::string>(attribute(reader, *reader.lookup(id), "value").data) ==
                  "copied input",
              "embedded attributes own copied input");
    for (const auto id : {event, empty_event}) {
        unsigned count = 0;
        reader.relations(id, vt::Direction::outgoing, [&](const auto& relation) {
            ++count;
            check(relation.to == (id == event ? stage : empty_stage),
                  "relation endpoint transport");
            check(relation.data.size() == (id == event ? 1U : 0U), "relation attribute transport");
            if (id == event)
                check(std::get<std::string>(relation.data.front().data.data) == "copied input",
                      "relation attributes own copied input");
            return vt::Flow::next;
        });
        check(count == 1, "relation count");
    }
}

int main(int argc, char** argv) {
    try {
        if (argc == 4 && std::string_view(argv[1]) == "--lifecycle") {
            lifecycle(argv[2], std::string_view(argv[3]) == "inline" ? vt::ExecutionMode::inline_
                                                                     : vt::ExecutionMode::threaded);
            return 0;
        }
        if (argc == 3 && std::string_view(argv[1]) == "--read-lifecycle") {
            check_lifecycle(argv[2], vt::RecordId(1), vt::RecordId(3), vt::RecordId(5),
                            vt::RecordId(23), vt::RecordId(25));
            return 0;
        }
        const auto root = std::filesystem::path(argc > 1 ? argv[1] : "tracker-fixtures");
        std::filesystem::create_directories(root);
        for (auto mode : {vt::ExecutionMode::inline_, vt::ExecutionMode::threaded}) {
            const auto name = mode == vt::ExecutionMode::inline_ ? "inline" : "threaded";
            lifecycle(root / (std::string(name) + ".vtr"), mode);
            populations(root / (std::string(name) + "-population.vtr"), mode);
            embedded_transport(root / (std::string(name) + "-embedded.vtr"), mode);
            handle_isolation(root / (std::string(name) + "-handles.vtr"), mode);
        }
        producer_migration(root / "migration.vtr");
        steady_capture(root / "steady.vtr");
    } catch (const std::exception& error) {
        count_allocations = false;
        std::cerr << error.what() << '\n';
        return 1;
    }
}
