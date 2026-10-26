#include "stream.h"

#include <atomic>
#include <barrier>
#include <filesystem>
#include <iostream>
#include <latch>
#include <stdexcept>
#include <thread>

using namespace vtr::detail::engine;

void check(bool condition, const char* message) {
    if (!condition) {
        throw std::runtime_error(message);
    }
}

template <class F> void rejects(F&& action) {
    try {
        action();
    } catch (const vtr::detail::wire::Error&) {
        return;
    }
    throw std::runtime_error("invalid producer operation accepted");
}

void skew_and_barrier(const std::filesystem::path& directory) {
    const auto path = directory / "skew.vtr";
    Stream stream(path, {.record_limit = 17}, {.queue_bytes = 256, .pending_bytes = 1024});
    const auto generator = stream.control(
        [](Writer& writer) { return writer.add_generator(none, "requests", "TRANSACTION"); });
    const auto text = stream.control(
        [](Writer& writer) { return writer.add_generator(none, "messages", "LOG"); });
    const auto callsite = stream.control(
        [](Writer& backend) { return backend.add_callsite("{}", backend.bind<std::string>()); });
    const auto fast_signal = stream.control([](Writer& writer) {
        return writer.add_var(none, "fast", {SignalKind::bits, 32, 2}).second;
    });
    const auto slow_signal = stream.control([](Writer& writer) {
        return writer.add_var(none, "slow", {SignalKind::bits, 32, 2}).second;
    });
    const auto fast = stream.add_producer();
    const auto slow = stream.add_producer();
    std::latch created(1);
    std::barrier barrier(2);
    RecordId first_fast = 0, first_slow = 0, borrowed = 0;
    constexpr unsigned count = 1000;
    std::exception_ptr fast_error, slow_error;
    std::thread a([&] {
        try {
            stream.use_producer(fast);
            for (unsigned i = 0; i < count; ++i) {
                stream.advance_time(100 + i);
                const auto id = stream.begin(generator, 100 + i);
                if (!i) {
                    first_fast = id;
                    created.count_down();
                }
                stream.emit(fast_signal, i);
                stream.end(id, 100 + i);
            }
            stream.publish();
            barrier.arrive_and_wait();
            stream.finish_producer();
        } catch (...) {
            fast_error = std::current_exception();
            barrier.arrive_and_drop();
        }
    });
    std::thread b([&] {
        try {
            stream.use_producer(slow);
            created.wait();
            first_slow = stream.begin(generator, 0);
            stream.emit(slow_signal, 73);
            auto value = capture(std::string("borrowed"));
            borrowed = stream.log(text, callsite, 0, value, first_slow);
            value.assign(value.size(), 0xff);
            stream.publish();
            barrier.arrive_and_wait();
            stream.advance_time(1);
            stream.end(first_slow, 1);
            stream.finish_producer();
        } catch (...) {
            slow_error = std::current_exception();
            barrier.arrive_and_drop();
        }
    });
    a.join();
    b.join();
    if (fast_error) {
        std::rethrow_exception(fast_error);
    }
    if (slow_error) {
        std::rethrow_exception(slow_error);
    }
    stream.close();
    check(first_fast < first_slow, "fixture did not reverse identity and recording-time order");
    Reader reader(path);
    check(reader.validate().records == count + 2, "skew lost records or identities");
    check(reader.transaction(first_fast)->begin == 100 &&
              reader.transaction(first_slow)->begin == 0,
          "timestamp order changed identities");
    check(decode<std::string>(reader.lookup(borrowed)->payload()) == "borrowed",
          "stream borrowed mutable input");
    Timestamp previous = 0;
    unsigned observations = 0;
    reader.changes(fast_signal, 0, UINT64_MAX, [&](Timestamp time, Id, const SignalValue& value) {
        check(time >= previous, "waveform timestamps are unordered");
        check(time == 100 + value.as_u64(), "waveform update has another batch's timestamp");
        previous = time;
        ++observations;
        return true;
    });
    check(observations == count, "skew lost waveform values");
    check(
        vtr::detail::wire::SignalValue{{SignalKind::bits, 32, 2}, reader.value_at(slow_signal, 100)}
                .as_u64() == 73,
        "slow producer's earlier sample was lost");
    const auto stats = stream.stream_stats();
    check(stats.spilled_bytes > 0 && stats.submitted == stats.completed,
          "tiny queues did not exercise spill and final drain");
    check(stats.waits <= stats.submitted && (!stats.waits || stats.wait_nanoseconds),
          "publication stalls were counted as individual spin iterations");
}

void migration_and_pause(const std::filesystem::path& directory) {
    const auto path = directory / "migration.vtr";
    Stream stream(path);
    const auto generator = stream.control(
        [](Writer& writer) { return writer.add_generator(none, "requests", "TRANSACTION"); });
    const auto producer = stream.add_producer();
    stream.use_producer(producer);
    stream.advance_time(10);
    auto id = stream.begin(generator, 10);
    stream.release_producer();
    std::thread worker([&] {
        stream.use_producer(producer);
        check(stream.time() == 10, "producer migration lost its time");
        stream.advance_time(11);
        stream.end(id, 11);
        stream.release_producer();
    });
    worker.join();
    stream.use_producer(producer);
    rejects([&] { stream.advance_time(9); });
    stream.flush();
    stream.advance_time(11);
    auto resumed = stream.begin(generator, 11);
    stream.end(resumed, 11);
    stream.finish_producer();
    rejects([&] { stream.use_producer(producer); });
    stream.close();
    Reader reader(path);
    check(reader.validate().records == 2 && reader.transaction(id)->end == 11 &&
              reader.transaction(resumed)->begin == 11,
          "pause/resume changed transaction lifetime");
}

void reject_conflicting_owners(const std::filesystem::path& directory) {
    const auto path = directory / "ownership.vtr";
    Stream stream(path);
    auto signal = stream.control([](Writer& writer) {
        return writer.add_var(none, "signal", {SignalKind::bits, 1, 2}).second;
    });
    const auto first = stream.add_producer();
    const auto second = stream.add_producer();
    stream.use_producer(first);
    stream.emit(signal, 1);
    stream.finish_producer();
    stream.use_producer(second);
    stream.emit(signal, 0);
    stream.finish_producer();
    rejects([&] { stream.close(); });
    rejects([&] { stream.emit(signal, 1); });
    stream.seal();
    check(Reader(path).incomplete(), "ownership violation produced a complete file");
}

void transfer_tracing_ownership(const std::filesystem::path& directory) {
    const auto path = directory / "transfer.vtr";
    Stream stream(path);
    auto generator = stream.control(
        [](Writer& backend) { return backend.add_generator(none, "request", "TRANSACTION"); });
    auto signal = stream.control([](Writer& backend) {
        return backend.add_var(none, "signal", {SignalKind::bits, 1, 2}).second;
    });
    auto key = stream.control([](Writer& backend) { return backend.intern("value"); });
    auto first = stream.add_producer(), second = stream.add_producer();
    stream.use_producer(first);
    auto tx = stream.begin(generator, 100);
    stream.emit(signal, 1);
    stream.advance_time(1);
    stream.release_producer();
    stream.use_producer(second);
    stream.advance_time(1);
    stream.attribute(tx, key, Value(std::uint64_t(73)));
    stream.emit(signal, 0);
    stream.end(tx, 101);
    stream.finish_producer();
    stream.use_producer(first);
    stream.finish_producer();
    stream.close();
    Reader reader(path);
    const auto record = reader.transaction(tx);
    check(record && record->begin == 100 && record->end == 101 && record->attributes.size() == 1 &&
              record->attributes[0].value.number == 73,
          "tracing-owner transfer lost transaction state");
    const auto sample = [&](Timestamp time) {
        return vtr::detail::wire::SignalValue{{SignalKind::bits, 1, 2},
                                              reader.value_at(signal, time)}
            .as_u64();
    };
    check(sample(0) == 1 && sample(1) == 0, "tracing-owner transfer changed waveform time");
    (void)reader.validate();

    Stream invalid(directory / "transaction-ownership.vtr");
    generator = invalid.control(
        [](Writer& backend) { return backend.add_generator(none, "request", "TRANSACTION"); });
    first = invalid.add_producer();
    second = invalid.add_producer();
    invalid.use_producer(first);
    tx = invalid.begin(generator, 0);
    invalid.release_producer();
    invalid.use_producer(second);
    invalid.end(tx, 1);
    invalid.finish_producer();
    invalid.use_producer(first);
    invalid.finish_producer();
    rejects([&] { invalid.close(); });
    invalid.seal();
    check(Reader(directory / "transaction-ownership.vtr").incomplete(),
          "same-time transaction ownership conflict finalized output");
}

void ownership_slot_collisions(const std::filesystem::path& directory) {
    const auto path = directory / "ownership-slot-collisions.vtr";
    Stream stream(path, {.record_limit = 11}, {.queue_bytes = 4096, .pending_bytes = 8192});
    const auto generator = stream.control(
        [](Writer& backend) { return backend.add_generator(none, "request", "TRANSACTION"); });
    const auto stage_generator = stream.control(
        [](Writer& backend) { return backend.add_generator(none, "stage", "STAGE"); });
    const auto log_generator = stream.control(
        [](Writer& backend) { return backend.add_generator(none, "filler", "LOG"); });
    const auto callsite =
        stream.control([](Writer& backend) { return backend.add_callsite("filler", 0); });
    const auto key = stream.control([](Writer& backend) { return backend.intern("value"); });
    const auto later = stream.control([](Writer& backend) { return backend.intern("later"); });
    const auto first = stream.add_producer(), second = stream.add_producer();
    std::array<RecordId, 3> owners, stages;
    stream.use_producer(first);
    for (unsigned i = 0; i < owners.size(); ++i) {
        if (i)
            for (unsigned n = 0; n < 1022; ++n)
                stream.log(log_generator, callsite, 0);
        owners[i] = stream.begin(generator, i);
        stages[i] = stream.begin_stage(stage_generator, owners[i], i);
        check(owners[i] == 1 + i * 1024 && stages[i] == 2 + i * 1024,
              "ownership collision fixture lost its identifier spacing");
    }
    for (unsigned i : {1u, 2u, 0u})
        stream.attribute(owners[i], key, Value(std::uint64_t(100 + i)));
    stream.end(owners[0], 10);
    stream.end_stage(stages[1], 11);
    stream.end(owners[1], 12);
    stream.advance_time(1);
    stream.release_producer();
    stream.use_producer(second);
    stream.advance_time(1);
    stream.attribute(owners[2], later, Value(std::uint64_t(200)));
    stream.end_stage(stages[2], 13);
    stream.end(owners[2], 14);
    stream.finish_producer();
    stream.use_producer(first);
    stream.finish_producer();
    stream.close();

    Reader reader(path);
    check(reader.validate().records == 2050, "colliding tracing owners lost records");
    for (unsigned i = 0; i < owners.size(); ++i) {
        const auto owner = reader.transaction(owners[i]);
        const auto stage = reader.lookup(stages[i]);
        check(owner && owner->attributes.size() == (i == 2 ? 2 : 1) &&
                  owner->attributes[0].key == key && owner->attributes[0].value.number == 100 + i,
              "colliding tracing owners changed identity or attributes");
        check(stage && stage->parent() == owners[i] && stage->end() == (i == 0 ? 10 : 9 + i * 2),
              "colliding tracing owners changed explicit or implicit stage closure");
        if (i == 2)
            check(owner->attributes[1].key == later && owner->attributes[1].value.number == 200,
                  "tracing ownership transfer lost colliding owner state");
    }
}

void startup_and_control_shutdown(const std::filesystem::path& directory) {
    const auto path = directory / "control.vtr";
    Stream stream(path);
    const auto generator = stream.control(
        [](Writer& backend) { return backend.add_generator(none, "requests", "TRANSACTION"); });
    const auto producer = stream.add_producer();
    std::latch start(1);
    std::exception_ptr failure;
    std::thread writer([&] {
        try {
            start.wait();
            stream.use_producer(producer);
            for (unsigned i = 0; i < 200; ++i) {
                stream.advance_time(i);
                auto id = stream.begin(generator, i);
                stream.end(id, i);
            }
            stream.finish_producer();
        } catch (...) {
            failure = std::current_exception();
        }
    });
    start.count_down();
    for (unsigned i = 0; i < 100; ++i) {
        stream.control([&](Writer& backend) { backend.intern("key" + std::to_string(i)); });
    }
    writer.join();
    if (failure) {
        std::rethrow_exception(failure);
    }
    std::latch observing(1);
    std::thread observer([&] {
        observing.count_down();
        try {
            for (;;) {
                (void)stream.stats();
            }
        } catch (const vtr::detail::wire::Error&) {
        }
    });
    observing.wait();
    stream.close();
    observer.join();
    check(Reader(path).validate().records == 200, "startup/shutdown lost accepted observations");
}

void exclusive_attachment(const std::filesystem::path& directory) {
    Stream first(directory / "attachment-first.vtr"), second(directory / "attachment-second.vtr");
    const auto a = first.add_producer(), b = first.add_producer();
    const auto other = second.add_producer();
    const auto cannot_attach = [&](Stream::ProducerId producer) {
        bool rejected = false;
        std::thread contender([&] {
            try {
                first.use_producer(producer);
                first.release_producer();
            } catch (const vtr::detail::wire::Error&) {
                rejected = true;
            }
        });
        contender.join();
        check(rejected, "attached producer could be claimed by another thread");
    };
    first.use_producer(a);
    cannot_attach(a);
    second.use_producer(other);
    first.use_producer(b);
    cannot_attach(b);
    std::latch attached(1), release(1);
    std::thread migrated([&] {
        first.use_producer(a);
        attached.count_down();
        release.wait();
        first.finish_producer();
    });
    attached.wait();
    first.release_producer();
    rejects([&] { first.close(); });
    release.count_down();
    migrated.join();
    first.use_producer(b);
    first.finish_producer();
    first.close();
    second.close();
    (void)Reader(directory / "attachment-first.vtr").validate();
    (void)Reader(directory / "attachment-second.vtr").validate();
}

void oversized_owned_payloads(const std::filesystem::path& directory) {
    const auto path = directory / "oversized.vtr";
    Stream stream(path, {}, {.queue_bytes = 128, .pending_bytes = 512});
    const auto generator = stream.control(
        [](Writer& backend) { return backend.add_generator(none, "messages", "LOG"); });
    const auto callsite = stream.control(
        [](Writer& backend) { return backend.add_callsite("{}", backend.bind<std::string>()); });
    const auto fast = stream.add_producer(), slow = stream.add_producer();
    stream.use_producer(fast);
    std::vector<RecordId> ids;
    for (unsigned i = 0; i < 40; ++i) {
        stream.advance_time(i + 1);
        const auto value = std::string(i % 2 ? 4096 : 7, static_cast<char>('a' + i % 26));
        auto payload = capture(value);
        ids.push_back(stream.log(generator, callsite, i + 1, payload));
        payload.assign(payload.size(), 0xff);
    }
    stream.release_producer();
    stream.use_producer(slow);
    stream.finish_producer();
    stream.use_producer(fast);
    stream.finish_producer();
    stream.close();
    Reader reader(path);
    for (unsigned i = 0; i < ids.size(); ++i) {
        const auto record = reader.lookup(ids[i]);
        check(record && record->begin() == i + 1 &&
                  decode<std::string>(record->payload()) ==
                      std::string(i % 2 ? 4096 : 7, static_cast<char>('a' + i % 26)),
              "oversized queue transport lost owned payload or timestamp");
    }
    check(reader.validate().records == ids.size() && stream.stream_stats().spilled_bytes > 0,
          "oversized queue transport did not exercise bounded staging and replay");
}

void spooled_scalar_attributes(const std::filesystem::path& directory) {
    const auto path = directory / "spooled-scalar-attributes.vtr";
    Stream stream(path, {.record_limit = 7}, {.queue_bytes = 64, .pending_bytes = 64});
    const auto generator = stream.control(
        [](Writer& writer) { return writer.add_generator(none, "request", "TRANSACTION"); });
    const auto stage_generator =
        stream.control([](Writer& writer) { return writer.add_generator(none, "stage", "STAGE"); });
    const auto tags = std::array{Tag::null, Tag::boolean, Tag::i8,  Tag::i16,  Tag::i32,
                                 Tag::i64,  Tag::u8,      Tag::u16, Tag::u32,  Tag::u64,
                                 Tag::f32,  Tag::f64,     Tag::str, Tag::time, Tag::pointer};
    std::vector<Id> keys;
    const auto literal = stream.control([&](Writer& writer) {
        for (unsigned i = 0; i <= tags.size(); ++i)
            keys.push_back(writer.intern("attribute" + std::to_string(i)));
        return writer.intern("literal");
    });
    const auto fast = stream.add_producer(), slow = stream.add_producer();
    stream.use_producer(fast);
    stream.advance_time(100);
    std::vector<Attributes> expected;
    std::vector<std::pair<RecordId, RecordId>> ids;
    for (unsigned i = 0; i < 128; ++i) {
        const auto owner = stream.begin(generator, 100);
        stream.span_kind(owner, 5);
        Attributes attrs;
        for (unsigned j = 0; j < tags.size(); ++j) {
            Value value;
            value.tag = tags[j];
            switch (value.tag) {
            case Tag::null:
                break;
            case Tag::boolean:
                value.number = i % 2;
                break;
            case Tag::i8:
                value.number = std::uint64_t(std::int64_t(-128));
                break;
            case Tag::i16:
                value.number = std::uint64_t(std::int64_t(-32768));
                break;
            case Tag::i32:
                value.number = std::uint64_t(std::int64_t(INT32_MIN));
                break;
            case Tag::i64:
                value.number = std::uint64_t(INT64_MIN);
                break;
            case Tag::u8:
                value.number = UINT8_MAX;
                break;
            case Tag::u16:
                value.number = UINT16_MAX;
                break;
            case Tag::u32:
                value.number = UINT32_MAX;
                break;
            case Tag::f32:
                value.number = 0x7fc12345;
                break;
            case Tag::f64:
                value.number = 0xfff8000000000011;
                break;
            case Tag::str:
                value.literal = literal;
                break;
            default:
                value.number = UINT64_MAX - i;
                break;
            }
            stream.attribute(owner, keys[j], value);
            attrs.push_back({keys[j], value});
        }
        std::array<std::uint8_t, 80> bytes;
        bytes.fill(static_cast<std::uint8_t>(i));
        auto value = Value::bytes(bytes);
        stream.attribute(owner, keys.back(), value);
        attrs.push_back({keys.back(), value});
        value.payload.assign(value.payload.size(), 0xff);
        const auto stage = stream.begin_stage(stage_generator, owner, 100);
        stream.end_stage(stage, 100);
        stream.end(owner, 100, 4);
        ids.emplace_back(owner, stage);
        expected.push_back(std::move(attrs));
    }
    stream.flush();
    check(stream.stream_stats().spilled_bytes > 0 && !stream.stats().active_owners,
          "future attribute packets did not wait in bounded staging");
    stream.finish_producer();
    stream.use_producer(slow);
    stream.advance_time(101);
    stream.finish_producer();
    stream.close();
    Reader reader(path);
    check(reader.validate().records == ids.size() * 2, "attribute replay lost owners or stages");
    for (unsigned i = 0; i < ids.size(); ++i) {
        const auto owner = reader.transaction(ids[i].first);
        const auto stage = reader.lookup(ids[i].second);
        check(owner && owner->span_kind == 5 && owner->status == 4 &&
                  owner->attributes.size() == expected[i].size() && stage &&
                  stage->parent() == owner->id && stage->begin() == 100 && stage->end() == 100,
              "attribute replay changed record identities or classifications");
        for (unsigned j = 0; j < expected[i].size(); ++j) {
            const auto& actual = owner->attributes[j];
            const auto& wanted = expected[i][j];
            check(actual.key == wanted.key && actual.value.tag == wanted.value.tag &&
                      actual.value.number == wanted.value.number &&
                      actual.value.literal == wanted.value.literal &&
                      actual.value.payload == wanted.value.payload,
                  "attribute replay changed numeric bits, string IDs or owned bytes");
        }
    }
    const auto stats = stream.stream_stats();
    check(stats.submitted == stats.completed, "attribute transport did not finish every operation");
}

void register_after_direct_recording(const std::filesystem::path& directory) {
    const auto path = directory / "late-producer.vtr";
    Stream stream(path);
    const auto signal = stream.control([](Writer& backend) {
        return backend.add_var(none, "signal", {SignalKind::bits, 8, 2}).second;
    });
    const auto first = stream.add_producer();
    stream.use_producer(first);
    stream.advance_time(10);
    stream.emit(signal, 10);
    stream.advance_time(11);
    stream.flush();
    stream.release_producer();
    rejects([&] { (void)stream.add_producer(10); });
    const auto second = stream.add_producer(11);
    stream.use_producer(second);
    stream.emit(signal, 11);
    stream.finish_producer();
    stream.use_producer(first);
    stream.finish_producer();
    stream.close();
    Reader reader(path);
    unsigned changes = 0;
    reader.changes(signal, 0, UINT64_MAX, [&](Timestamp time, Id, const SignalValue& value) {
        check(time == 10 + changes && value.as_u64() == time,
              "registering another producer moved completed waveform time");
        ++changes;
        return true;
    });
    check(changes == 2, "late producer registration lost waveform changes");
    (void)reader.validate();
}

int main(int argc, char** argv) {
    try {
        if (argc != 2) {
            throw std::runtime_error("usage: stream_test DIRECTORY");
        }
        std::filesystem::create_directories(argv[1]);
        skew_and_barrier(argv[1]);
        migration_and_pause(argv[1]);
        reject_conflicting_owners(argv[1]);
        transfer_tracing_ownership(argv[1]);
        ownership_slot_collisions(argv[1]);
        startup_and_control_shutdown(argv[1]);
        exclusive_attachment(argv[1]);
        oversized_owned_payloads(argv[1]);
        register_after_direct_recording(argv[1]);
        spooled_scalar_attributes(argv[1]);
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
