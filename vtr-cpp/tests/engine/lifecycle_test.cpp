#include "engine/vtr.h"
#include "format.h"
#include <filesystem>
#include <fstream>
#include <iostream>
#include <source_location>

using namespace vtr::detail::engine;
namespace fs = std::filesystem;

struct Request {
    std::uint64_t serial, address;
    std::uint32_t opcode;
};

struct Visit {
    std::uint32_t port;
    std::string reason;
};

template <> struct vtr::detail::engine::Record<Request> {
    static constexpr std::string_view name = "Request";

    static auto fields() {
        return std::tuple{field("serial", &Request::serial), field("address", &Request::address),
                          field("opcode", &Request::opcode)};
    }
};

template <> struct vtr::detail::engine::Record<Visit> {
    static constexpr std::string_view name = "Visit";

    static auto fields() {
        return std::tuple{field("port", &Visit::port), field("reason", &Visit::reason)};
    }
};

void check(bool good, const char* message) {
    if (!good) {
        throw std::runtime_error(message);
    }
}

template <class F>
void rejects(F&& action, std::source_location where = std::source_location::current()) {
    try {
        action();
    } catch (const vtr::detail::wire::Error&) {
        return;
    }
    throw std::runtime_error("invalid operation accepted at line " + std::to_string(where.line()));
}

void hierarchy(const fs::path& dir, unsigned workers = 0) {
    auto path = dir / (workers ? "hierarchy-async.vtr" : "hierarchy.vtr");
    RecordId grid, warp, instruction, child, local_event, local_stage, visit_a, visit_b,
        implicit_visit, event_a, event_b, message;
    GeneratorId stage, event, log, clock_a, clock_b, instruction_gen;
    Id signal, wire_event;
    std::uint64_t max_id;
    auto args = capture(Visit{2, "wait Δ"});
    {
        Writer writer(path, {.codec = vtr::detail::wire::Codec::lz4,
                             .page_bytes = 256,
                             .block_bytes = 2048,
                             .record_limit = 4,
                             .workers = workers,
                             .queue_jobs = 3,
                             .queue_bytes = 4096});
        writer.metadata().timescale = -9;
        auto request_type = writer.bind<Request>(), visit_type = writer.bind<Visit>();
        check(request_type == writer.bind<Request>() && visit_type == writer.bind<Visit>(),
              "C++ datatype binding was redeclared");
        auto top = writer.add_scope(none, "gpu");
        auto grid_gen = writer.add_generator(top, "grid", "GRID"),
             warp_gen = writer.add_generator(top, "warp", "WARP");
        instruction_gen = writer.add_generator(top, "instruction", "INSTRUCTION", request_type);
        stage = writer.add_generator(top, "execute_slot", "STAGE", visit_type);
        auto other_stage = writer.add_generator(top, "writeback", "STAGE", visit_type);
        event = writer.add_generator(top, "conflict", "EVENT", visit_type);
        log = writer.add_generator(top, "request port={} reason={}", "LOG", visit_type);
        writer.generator_attribute(log, writer.intern("log.severity"), Value(std::uint64_t(3)));
        clock_a = writer.add_clock(top, "clk_a");
        clock_b = writer.add_clock(top, "clk_b");
        writer.associate_clock(instruction_gen, clock_a);
        writer.associate_clock(stage, clock_a);
        writer.associate_clock(event, clock_b);
        signal = writer.add_var(top, "counter", {SignalKind::bits, 8, 2}).second;
        wire_event = writer.add_var(top, "pulse", {SignalKind::bits, 1, 2}, 0).second;
        writer.add_alias(top, "counter_alias", signal);
        auto note = writer.intern("local_note"), wait = writer.intern("local_wait"),
             lane = writer.intern("main"), dependency = writer.intern("depends_on");
        auto serial = writer.intern("serial"), label = writer.intern("vtr.label");
        grid = writer.begin(grid_gen, 0);
        warp = writer.begin(warp_gen, 1, grid);
        instruction = writer.begin(instruction_gen, 3, capture(Request{11, 0x80000000, 7}), warp);
        local_event = writer.embedded_event(instruction, 999, note);
        local_stage = writer.embedded_stage(instruction, wait, lane, 3);
        visit_a = writer.begin_stage(stage, instruction, 4, args);
        visit_b = writer.begin_stage(stage, instruction, 4, args);
        implicit_visit = writer.begin_stage(other_stage, instruction, 100, args);
        event_a = writer.event(event, instruction, 5, args);
        event_b = writer.event(event, instruction, 5, args);
        auto source_args = args;
        message = writer.log(log, 5, source_args, instruction);
        std::fill(source_args.begin(), source_args.end(), 0);
        child = writer.begin(instruction_gen, 30, instruction);
        rejects([&] { writer.end(child, 200); });
        writer.payload(child, capture(Request{12, 0x80000004, 8}));
        rejects([&] { writer.attribute(instruction, serial, Value(std::uint64_t(11))); });
        writer.attribute(instruction, label, Value::text("instruction 11"));
        rejects([&] { writer.attribute(instruction, label, Value::text("duplicate")); });
        writer.end(grid, 2);
        rejects([&] { writer.begin(grid_gen, 5, grid); });
        writer.end_stage(visit_a, 10);
        writer.end_stage(visit_b, 12);
        writer.end(instruction, 20, 3);
        writer.end(child, 200);
        writer.end(warp, 15);
        rejects([&] { writer.event(event, instruction, 21, args); });
        rejects([&] { writer.end_stage(implicit_visit, 101); });
        writer.relate(dependency, local_event, visit_a);
        writer.relate(dependency, event_a, child);
        writer.clock_run(clock_a, 0, 2);
        writer.clock_stop(clock_a, 7);
        writer.clock_run(clock_a, 10, 3);
        writer.clock_stop(clock_a, 17);
        writer.clock_run(clock_a, 18, 5);
        rejects([&] { writer.clock_run(clock_a, 18, 5); });
        rejects([&] { writer.clock_run(clock_b, 0, 0); });
        writer.clock_run(clock_b, 1, 4);
        writer.clock_stop(clock_b, 1);
        writer.clock_run(clock_b, 21, 1);
        for (unsigned t = 0; t <= 25; ++t) {
            writer.set_time(t);
            writer.emit(signal, t / 3);
            writer.emit(wire_event, 1);
        }
        writer.close();
        max_id = writer.stats().max_created_id;
        check(writer.stats().active_owners == 0 && writer.stats().active_stages == 0,
              "close retained active owner/stage handles");
    }
    Reader reader(path, {.verify_checksums = true, .owner_cache_entries = 1});
    auto report = reader.validate();
    check(!report.incomplete_file && report.missing.empty() && report.records == max_id &&
              report.relations == 2,
          "complete file failed global identity validation");
    auto expansion = reader.expand(instruction);
    check(expansion.complete && expansion.located.size() == 5,
          "owner expansion lost activity or included logs");
    auto visit = reader.lookup(visit_a);
    check(visit && visit->kind() == RecordKind::stage && visit->parent() == instruction &&
              visit->generator() == stage && visit->begin() == 4 && visit->end() == 10,
          "located identity differs from owner view");
    check(reader.lookup(event_a)->id() != reader.lookup(event_b)->id(),
          "coincident identical events were merged");
    auto implicit = reader.lookup(implicit_visit);
    check(implicit->begin() == 100 && implicit->end() == 100,
          "owner finalization did not clamp stage end");
    auto embedded = reader.lookup(local_event);
    check(embedded && embedded->kind() == RecordKind::embedded_event && !embedded->generator() &&
              embedded->parent() == instruction && embedded->begin() == 999,
          "embedded identity/location changed");
    auto local = reader.lookup(local_stage);
    check(local->kind() == RecordKind::embedded_stage && local->end() == 20 && !local->stage()->end,
          "open embedded stage lost absent-end distinction");
    check(reader.logs(instruction).size() == 1 &&
              reader.lookup(message)->payload().size() == args.size(),
          "associated diagnostic navigation failed");
    check(reader.lookup(message)->located()->payload == args,
          "writer retained mutable producer arguments");
    auto children = reader.children(instruction);
    check(children.size() == 1 && children[0].id == child && children[0].end == 200,
          "ordinary child finalized after parent was lost");
    check(reader.children(grid)[0].id == warp, "logical hierarchy traversal failed");
    unsigned located = 0;
    reader.records({.generator = stage, .parent = {}, .begin = 5, .end = 5}, [&](const auto&) {
        ++located;
        return true;
    });
    check(located == 2, "overlapping resource visits were discarded");
    unsigned owner_window = 0;
    reader.transactions({.id = instruction, .begin = 999, .end = 999}, [&](const auto&) {
        ++owner_window;
        return true;
    });
    check(owner_window == 0, "embedded time changed owner-interval query semantics");
    auto held = reader.transaction(instruction);
    reader.transaction(grid);
    reader.clear_cache();
    check(held->events[0].id == local_event && local->stage()->id == local_stage &&
              local->end() == 20,
          "cache eviction invalidated held results");
    check(reader.clock_for(instruction_gen) == clock_a && reader.clock_for(event) == clock_b,
          "clock association lost source domain");
    check(!reader.cycle_at(clock_b, 0) && reader.cycle_at(clock_b, 1) == 0 &&
              reader.cycle_at(clock_b, 21) == 1 && reader.cycle_at(clock_b, 25) == 5,
          "single-edge or second-domain cycle numbering failed");
    check(reader.cycle_at(clock_a, 8) == 3 && reader.cycle_at(clock_a, 10) == 4 &&
              reader.cycle_at(clock_a, 17) == 6 && reader.cycle_at(clock_a, 25) == 8,
          "paused/frequency-changing clock cycles changed");
    unsigned changes = 0, pulses = 0;
    reader.changes(0, UINT64_MAX, [&](auto, auto id, const auto&) {
        if (id == signal) {
            ++changes;
        }
        if (id == wire_event) {
            ++pulses;
        }
        return true;
    });
    check(changes == 9 && pulses == 26 && reader.find_signal("gpu.counter_alias") == signal,
          "waveform integration lost state/event semantics");
    Reader bounded(path, {.owner_cache_entries = 128, .owner_cache_bytes = 1000});
    for (unsigned i = 0; i < 100; ++i) {
        for (auto id : {grid, warp, instruction, child}) {
            bounded.transaction(id);
        }
        auto cache = bounded.cache_stats();
        check(cache.owner_entries <= 128 && cache.accounted_bytes <= 1000,
              "owner cache accounting drifted after eviction");
    }
    auto history = reader.load_signal(signal);
    reader.clear_cache();
    check(history->value_at(13).as_u64() == 4, "held waveform history was invalidated");
}

void long_owner(const fs::path& dir, unsigned workers = 0) {
    auto path = dir / (workers ? "long-owner-async.vtr" : "long-owner.vtr");
    RecordId owner;
    {
        Writer writer(path, {.page_bytes = 256,
                             .block_bytes = 1024,
                             .record_limit = 5,
                             .workers = workers,
                             .queue_jobs = 3,
                             .queue_bytes = 4096});
        auto type = writer.bind<Visit>();
        auto scope = writer.add_scope(none, "top");
        auto gen = writer.add_generator(scope, "owner", "TRANSACTION");
        auto event = writer.add_generator(scope, "event", "EVENT", type);
        auto log = writer.add_generator(scope, "log", "LOG", type);
        owner = writer.begin(gen, 0);
        auto data = capture(Visit{0, std::string(128, 'x')});
        for (unsigned i = 0; i < 5000; ++i) {
            writer.event(event, owner, 1, data);
            if (i % 10 == 0) {
                writer.log(log, 1, data, owner);
            }
        }
        writer.flush();
        auto stats = writer.stats();
        check(stats.active_owners == 1 && stats.active_stages == 0 && stats.record_blocks > 100 &&
                  stats.active_payload_bytes < 50000,
              "completed child payloads remain resident in the active owner");
        writer.end(owner, 2);
        writer.close();
    }
    Reader reader(path, {.verify_checksums = true});
    auto report = reader.validate();
    check(report.records == 5501 && reader.expand(owner).located.size() == 5000 &&
              reader.logs(owner).size() == 500,
          "long-owner expansion lost occurrences or mixed logs with activity");
}

void failed_close(const fs::path& dir) {
    auto path = dir / "missing-payload.vtr";
    {
        Writer writer(path);
        auto type = writer.bind<Request>();
        auto gen = writer.add_generator(none, "owner", "TRANSACTION", type),
             event = writer.add_generator(none, "event", "EVENT");
        auto owner = writer.begin(gen, 0);
        writer.event(event, owner, 0);
        rejects([&] { writer.close(); });
    }
    Reader incomplete(path, {.verify_checksums = true});
    auto report = incomplete.validate();
    check(incomplete.incomplete() && report.records == 1 && report.missing.size() == 1 &&
              report.missing[0].kind == MissingReference::Kind::parent &&
              report.missing[0].target == 1,
          "failed close fabricated a payload or hid the missing owner");
    auto unresolved = dir / "unresolved.vtr";
    {
        Writer writer(unresolved);
        auto gen = writer.add_generator(none, "owner", "TRANSACTION"),
             relation = writer.intern("dependency");
        auto owner = writer.begin(gen, 0);
        writer.end(owner, 1);
        writer.relate(relation, owner, 999);
        rejects([&] { writer.close(); });
        writer.seal();
    }
    Reader sealed(unresolved);
    auto references = sealed.validate();
    check(references.incomplete_file && references.missing.size() == 1 &&
              references.missing[0].kind == MissingReference::Kind::relation_destination &&
              references.missing[0].target == 999,
          "sealing hid an unresolved relation endpoint");
}

void deferred_stages(const fs::path& dir) {
    auto path = dir / "deferred-stages.vtr";
    RecordId root, ready, deferred, clock_span;
    auto payload = capture(Visit{17, "result"});
    {
        Writer writer(path);
        auto type = writer.bind<Visit>();
        auto owner = writer.add_generator(none, "owner", "TRANSACTION");
        auto stage = writer.add_generator(none, "stage", "STAGE", type);
        auto clock = writer.add_clock(none, "clock");
        root = writer.begin(owner, 0);
        ready = writer.begin_stage(stage, root, 3, payload);
        deferred = writer.begin_stage(stage, root, 4);
        rejects([&] { writer.end_stage(deferred, 99); });
        rejects([&] { writer.end(root, 99); });
        rejects([&] { writer.close(); });
        rejects([&] { writer.stage_payload(deferred, Bytes{0}); });
        check(writer.stats().active_stages == 2 && writer.stats().active_owners == 1,
              "rejected finalization mutated active stages");
        writer.stage_payload(deferred, payload);
        auto replacement = capture(Visit{18, "updated"});
        writer.stage_payload(ready, replacement);
        std::fill(replacement.begin(), replacement.end(), 0);
        rejects([&] { writer.stage_payload(ready, Bytes{0}); });
        clock_span = writer.clock_run(clock, 0, 2);
        writer.relate(writer.intern("cause"), clock_span, deferred);
        writer.clock_stop(clock, 10);
        writer.end(root, 10);
        writer.close();
    }
    Reader reader(path);
    check(!reader.validate().incomplete_file, "deferred stage trace incomplete");
    check(reader.lookup(clock_span)->kind() == RecordKind::clock, "clock handle does not resolve");
    auto expanded = reader.expand(root);
    check(expanded.complete && expanded.located.size() == 2,
          "deferred stages absent from expansion");
    for (const auto& stage : expanded.located) {
        auto value = decode<Visit>(stage.payload);
        check(stage.end == 10 &&
                  ((stage.id == ready && value.port == 18 && value.reason == "updated") ||
                   (stage.id == deferred && stage.payload == payload)),
              "deferred stage payload or rejected end changed final record");
    }
    std::vector<Relation> relations;
    reader.relations(clock_span, false, [&](const auto& relation) {
        relations.push_back(relation);
        return true;
    });
    check(relations.size() == 1 && relations[0].destination == deferred,
          "clock causal endpoint lost");
    auto failed = dir / "missing-stage-payload.vtr";
    {
        Writer writer(failed);
        auto type = writer.bind<Visit>();
        auto owner = writer.add_generator(none, "owner", "TRANSACTION");
        auto stage = writer.add_generator(none, "stage", "STAGE", type);
        auto event = writer.add_generator(none, "event", "EVENT");
        auto id = writer.begin(owner, 0);
        writer.begin_stage(stage, id, 1);
        writer.event(event, id, 2);
        rejects([&] { writer.close(); });
    }
    Reader incomplete(failed);
    auto report = incomplete.validate();
    check(report.incomplete_file && report.records == 1 && report.missing.size() == 1,
          "failed stage close fabricated incomplete observations");
}

void oversized_blocks(const fs::path& dir) {
    auto payload = capture(std::string(4096, 'x')), small = capture(std::string("small"));
    auto records = dir / "oversized-records.vtr";
    {
        Writer writer(records, {.block_bytes = 1024});
        auto owner = writer.add_generator(none, "owner", "TRANSACTION"),
             event = writer.add_generator(none, "event", "EVENT", 13);
        auto root = writer.begin(owner, 0);
        writer.event(event, root, 0, small);
        writer.event(event, root, 1, payload);
        writer.event(event, root, 2, small);
        writer.end(root, 3);
        writer.close();
    }
    Reader reader(records);
    auto expansion = reader.expand(1);
    check(reader.validate().records == 4 && expansion.complete &&
              expansion.owner->child_blocks.size() == 3,
          "oversized record shared a block");
    ContainerReader record_file(records);
    for (auto number : expansion.owner->child_blocks) {
        std::size_t count = 0;
        record_file.record_block(number)->query({}, [&](const auto&) {
            ++count;
            return true;
        });
        check(count == 1, "oversized record block has neighbors");
    }
    auto transactions = dir / "oversized-transactions.vtr";
    {
        Writer writer(transactions, {.block_bytes = 1024});
        auto gen = writer.add_generator(none, "owner", "TRANSACTION", 13);
        for (const auto* value : {&small, &payload, &small}) {
            auto id = writer.begin(gen, 0, *value);
            writer.end(id, 1);
        }
        writer.close();
    }
    ContainerReader tx_file(transactions);
    std::size_t blocks = 0;
    for (const auto& section : tx_file.sections()) {
        if (section.kind == 5) {
            ++blocks;
            std::size_t count = 0;
            tx_file.transaction_block(section.block_number)->transactions({}, [&](const auto&) {
                ++count;
                return true;
            });
            check(count == 1, "oversized transaction block has neighbors");
        }
    }
    check(blocks == 3 && Reader(transactions).validate().records == 3,
          "oversized transaction was not isolated");
}

void reader_limits(const fs::path& dir) {
    auto path = dir / "reader-limits.vtr";
    auto data = capture(std::vector<std::uint64_t>(512, 0));
    {
        Writer writer(path);
        auto type = writer.bind<std::vector<std::uint64_t>>();
        auto owner = writer.add_generator(none, "owner", "TRANSACTION", type),
             event = writer.add_generator(none, "event", "EVENT", type);
        auto id = writer.begin(owner, 0, data);
        writer.event(event, id, 1, data);
        writer.end(id, 2);
        writer.close();
    }
    Reader limited(path, {.max_payload_bytes = 4096});
    rejects([&] { limited.transaction(1); });
    rejects([&] { limited.lookup(2); });
    rejects([&] { limited.validate(); });
    Reader exact(path, {.verify_checksums = true, .max_payload_bytes = data.size()});
    check(exact.validate().records == 2 && exact.transaction(1)->payload == data,
          "configured owner payload limit was not propagated");
    auto record = exact.lookup(2);
    check(record && Bytes(record->payload().begin(), record->payload().end()) == data,
          "configured record payload limit was not propagated");
}

void reused_owner_slots(const fs::path& dir, unsigned workers) {
    const auto path = dir / (workers ? "reused-owners-async.vtr" : "reused-owners.vtr");
    constexpr unsigned count = 128;
    {
        Writer writer(path, {.record_limit = 3, .workers = workers});
        const auto type = writer.bind<Visit>();
        const auto typed = writer.add_generator(none, "typed", "TRANSACTION", type);
        const auto unit = writer.add_generator(none, "unit", "TRANSACTION");
        const auto key = writer.intern("note");
        const auto stage = writer.intern("stage");
        const auto lane = writer.intern("lane");
        for (unsigned first = 0; first < count; first += 2) {
            std::array<RecordId, 2> owners;
            for (unsigned offset = 0; offset < owners.size(); ++offset) {
                const auto i = first + offset;
                writer.assign_record_id(1 + std::uint64_t(i) * 1024);
                owners[offset] = i % 2 ? writer.begin(unit, i * 3)
                                       : writer.begin(typed, i * 3, capture(Visit{i, "payload"}));
                if (i % 3 == 0)
                    writer.attribute(owners[offset], key, Value::text("note " + std::to_string(i)));
                if (i % 3 == 0)
                    writer.span_kind(owners[offset], 5);
                if (i % 4 == 0)
                    writer.embedded_stage(owners[offset], stage, lane, i * 3 + 1);
            }
            for (unsigned offset = 0; offset < owners.size(); ++offset) {
                const auto i = first + offset;
                writer.end(owners[offset], i * 3 + 2, i % 5);
            }
            check(writer.stats().active_owners == 0, "completed owner remains active");
        }
        writer.close();
    }
    Reader reader(path, {.verify_checksums = true});
    reader.validate();
    unsigned seen = 0;
    reader.transactions({}, [&](const Transaction& tx) {
        const auto i = (tx.id - 1) / 1024;
        check(i < count && tx.id == i * 1024 + 1 && tx.begin == i * 3 && tx.end == i * 3 + 2 &&
                  tx.status == i % 5 && tx.span_kind == (i % 3 == 0 ? 5 : 0) && !tx.parent &&
                  tx.events.empty() && tx.child_blocks.empty() &&
                  tx.stages.size() == (i % 4 == 0) && tx.attributes.size() == (i % 3 == 0),
              "reused owner retained identity, classification or child data");
        if (i % 2)
            check(tx.payload.empty(), "unit owner retained a typed payload");
        else {
            const auto data = decode<Visit>(tx.payload);
            check(data.port == i && data.reason == "payload", "reused owner changed its payload");
        }
        if (!tx.attributes.empty())
            check(tx.attributes.front().value.payload ==
                      Value::text("note " + std::to_string(i)).payload,
                  "reused owner changed its attributes");
        ++seen;
        return true;
    });
    check(seen == count, "reused owner was duplicated or lost");
}

void reused_stage_lists(const fs::path& dir, unsigned workers) {
    const auto path = dir / ("reused-stage-lists-" + std::to_string(workers) + ".vtr");
    std::vector<LocatedRecord> expected;
    {
        Writer writer(path, {.record_limit = 13, .workers = workers});
        const auto owner = writer.add_generator(none, "owner", "TRANSACTION");
        const auto stage = writer.add_generator(none, "stage", "STAGE");
        const auto pinned = writer.begin(owner, 0);
        const auto pinned_stage = writer.begin_stage(stage, pinned, 0);
        const std::array counts{1u, 2u, 16u, 0u};
        for (unsigned i = 0; i < 96; ++i) {
            writer.assign_record_id(1025 + std::uint64_t(i) * 1024);
            const auto begin = std::uint64_t(i) * 10 + 1;
            const auto id = writer.begin(owner, begin);
            std::vector<RecordId> active;
            for (unsigned j = 0; j < counts[i % counts.size()]; ++j) {
                const auto record = writer.begin_stage(stage, id, begin + j);
                active.push_back(record);
                expected.push_back({record, stage, begin + j,
                                    j % 2 ? begin + 20 : begin + j + 3, id, {}});
            }
            for (unsigned j = 0; j < active.size(); j += 2)
                writer.end_stage(active[j], begin + j + 3);
            writer.end(id, begin + 20);
            for (auto record : active)
                rejects([&] { writer.end_stage(record, begin + 30); });
            check(writer.stats().active_owners == 1 && writer.stats().active_stages == 1,
                  "reused stage list retained completed activity");
        }
        writer.end(pinned, 2000);
        expected.push_back({pinned_stage, stage, 0, 2000, pinned, {}});
        writer.close();
    }
    Reader reader(path);
    reader.validate();
    std::vector<LocatedRecord> actual;
    reader.records({}, [&](const auto& record) {
        actual.push_back(record);
        return true;
    });
    const auto by_id = [](const auto& a, const auto& b) { return a.id < b.id; };
    std::sort(actual.begin(), actual.end(), by_id);
    std::sort(expected.begin(), expected.end(), by_id);
    check(actual == expected, "reused or displaced stage list changed owner, time or identity");
}

void unordered_attributes(const fs::path& dir, unsigned workers) {
    const auto path = dir / ("unordered-attributes-" + std::to_string(workers) + ".vtr");
    Writer writer(path, {.record_limit = 3, .workers = workers});
    const auto generator = writer.add_generator(none, "owner", "TRANSACTION");
    std::array<Id, 10> keys;
    for (unsigned i = 0; i < keys.size(); ++i)
        keys[i] = writer.intern("attribute" + std::to_string(i));
    std::array<Attributes, 64> expected;
    const std::array order{5u, 0u, 4u, 1u, 3u, 2u, 7u, 6u, 9u, 8u};
    const std::array counts{0u, 1u, 5u, 8u, 9u, 10u};
    for (unsigned first = 0; first < expected.size(); first += 32) {
        std::array<RecordId, 32> owners;
        for (unsigned offset = 0; offset < owners.size(); ++offset) {
            const auto index = first + offset;
            writer.assign_record_id(1 + index * 1024);
            owners[offset] = writer.begin(generator, index);
        }
        for (unsigned position = 0; position < order.size(); ++position) {
            for (unsigned remaining = owners.size(); remaining; --remaining) {
                const auto offset = remaining - 1;
                const auto index = first + offset;
                const auto count = counts[index % counts.size()];
                if (position >= count)
                    continue;
                const auto key = order[position];
                Value value(std::uint64_t(index * 100 + key));
                if (index % 7 == 0 && position + 1 == count)
                    value = Value::string(keys[0]);
                writer.attribute(owners[offset], keys[key], value);
                expected[index].push_back({keys[key], value});
            }
        }
        for (unsigned offset = 0; offset < owners.size(); ++offset) {
            const auto index = first + offset;
            for (const auto& attribute : expected[index])
                rejects([&] {
                    writer.attribute(owners[offset], attribute.key, Value(std::uint64_t(0)));
                });
            writer.end(owners[offset], index + 1);
        }
    }
    writer.close();
    Reader reader(path);
    reader.validate();
    for (unsigned i = 0; i < expected.size(); ++i) {
        const auto tx = reader.transaction(1 + i * 1024);
        check(bool(tx), "unordered attributes lost owner identity");
        Bytes actual, wanted;
        vtr::detail::wire::detail::encode_attrs(actual, tx->attributes);
        vtr::detail::wire::detail::encode_attrs(wanted, expected[i]);
        check(actual == wanted,
              "unordered attributes changed after duplicate rejection or slot reuse");
    }
}

void shared_text_lifecycle(const fs::path& dir, unsigned workers) {
    const auto path = dir / ("shared-text-" + std::to_string(workers) + ".vtr");
    const std::array<std::string, 3> values{std::string(2 << 20, 'a'), std::string(2 << 20, 'b'),
                                            std::string(2 << 20, 'c')};
    {
        Writer writer(path, {.page_bytes = 128,
                             .block_bytes = 1024,
                             .workers = workers,
                             .queue_jobs = 2,
                             .queue_bytes = 4096});
        const auto type = writer.bind<Visit>();
        const auto generator = writer.add_generator(none, "owner", "TRANSACTION", type);
        for (unsigned i = 0; i < 4; ++i) {
            const auto payload = capture(Visit{i, values[i % values.size()]});
            const auto id = writer.begin(generator, i, payload);
            writer.end(id, i + 1);
        }
        writer.close();
    }
    Reader reader(path);
    check(reader.validate().records == 4, "shared text lost its record inventory");
    std::size_t shared_bytes = 0;
    for (const auto& text : reader.strings())
        if (text.size() >= shared_text_minimum)
            shared_bytes += text.size();
    check(shared_bytes == 4 << 20, "shared text admission exceeded its file budget");
    for (unsigned i = 0; i < 4; ++i) {
        const auto owner = reader.transaction(i + 1);
        const auto value = decode<Visit>(owner->payload);
        check(value.port == i && value.reason == values[i % values.size()],
              "shared text changed after admission exhaustion or asynchronous encoding");
    }
    Reader limited(path, {.max_payload_bytes = 1 << 20});
    rejects([&] { limited.transaction(1); });
    if (!workers) {
        auto old = dir / "old-text-version.vtr";
        fs::copy_file(path, old, fs::copy_options::overwrite_existing);
        {
            std::fstream file(old, std::ios::binary | std::ios::in | std::ios::out);
            file.seekp(10);
            const char minor[] = {1, 0};
            file.write(minor, 2);
        }
        try {
            Reader obsolete(old);
            throw std::runtime_error("obsolete text format accepted");
        } catch (const vtr::detail::wire::Error& error) {
            check(error.code == 5, "obsolete text format returned another error");
        }
        const auto partial = dir / "partial-shared-text.vtr";
        fs::copy_file(path, partial, fs::copy_options::overwrite_existing);
        fs::resize_file(partial, fs::file_size(partial) - 12);
        const auto repaired = dir / "recovered-shared-text.vtr";
        recover_container(partial, repaired);
        Reader recovered(repaired);
        for (unsigned i = 0; i < 4; ++i)
            check(decode<Visit>(recovered.transaction(i + 1)->payload).reason ==
                      values[i % values.size()],
                  "recovery lost shared text references");
    }
}

void same_sections(const fs::path& a, const fs::path& b) {
    auto read = [](const fs::path& path) {
        std::ifstream f(path, std::ios::binary);
        return Bytes(std::istreambuf_iterator<char>(f), {});
    };
    auto first = read(a), second = read(b);
    check(first.size() > 32 && second.size() > 32, "missing comparison fixture");
    std::fill(first.begin() + 16, first.begin() + 32, 0);
    std::fill(second.begin() + 16, second.begin() + 32, 0);
    check(first == second,
          "worker encoding changed sections, declaration order or waveform predecessors");
}

int main(int argc, char** argv) {
    try {
        if (argc != 2) {
            throw std::runtime_error("usage: lifecycle_test DIRECTORY");
        }
        fs::create_directories(argv[1]);
        hierarchy(argv[1]);
        hierarchy(argv[1], 2);
        long_owner(argv[1]);
        long_owner(argv[1], 2);
        failed_close(argv[1]);
        deferred_stages(argv[1]);
        oversized_blocks(argv[1]);
        reader_limits(argv[1]);
        reused_owner_slots(argv[1], 0);
        reused_owner_slots(argv[1], 2);
        reused_stage_lists(argv[1], 0);
        reused_stage_lists(argv[1], 2);
        unordered_attributes(argv[1], 0);
        unordered_attributes(argv[1], 2);
        shared_text_lifecycle(argv[1], 0);
        shared_text_lifecycle(argv[1], 2);
        same_sections(fs::path(argv[1]) / "shared-text-0.vtr",
                      fs::path(argv[1]) / "shared-text-2.vtr");
        same_sections(fs::path(argv[1]) / "reused-stage-lists-0.vtr",
                      fs::path(argv[1]) / "reused-stage-lists-2.vtr");
        same_sections(fs::path(argv[1]) / "hierarchy.vtr",
                      fs::path(argv[1]) / "hierarchy-async.vtr");
        same_sections(fs::path(argv[1]) / "long-owner.vtr",
                      fs::path(argv[1]) / "long-owner-async.vtr");
        std::cout << "Validated writer lifecycle, full-file identities/relations/locators, "
                     "hierarchy/diagnostic navigation, implicit/open stage closure, "
                     "duplicate/overlapping activity, two changing/paused clocks, stable held "
                     "results and failed-close sealing\n";
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
