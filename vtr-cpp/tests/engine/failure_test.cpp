#include "engine/vtr.h"
#include "stream.h"
#include "vtr_writer.h"
#include <csignal>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <new>
#include <sys/resource.h>
#include <thread>

namespace {
thread_local std::int64_t fail_after = -1;

void allocate_point() {
    if (fail_after == 0) {
        fail_after = -1;
        throw std::bad_alloc();
    }
    if (fail_after > 0) {
        --fail_after;
    }
}
} // namespace

[[gnu::noinline]] void* operator new(std::size_t n) {
    allocate_point();
    if (auto p = std::malloc(n ? n : 1)) {
        return p;
    }
    throw std::bad_alloc();
}

[[gnu::noinline]] void* operator new[](std::size_t n) {
    return ::operator new(n);
}

void* operator new(std::size_t n, const std::nothrow_t&) noexcept {
    try {
        return ::operator new(n);
    } catch (...) {
        return nullptr;
    }
}

void* operator new[](std::size_t n, const std::nothrow_t& tag) noexcept {
    return ::operator new(n, tag);
}

void operator delete(void* p, const std::nothrow_t&) noexcept {
    std::free(p);
}

void operator delete[](void* p, const std::nothrow_t&) noexcept {
    std::free(p);
}

[[gnu::noinline]] void operator delete(void* p) noexcept {
    std::free(p);
}

[[gnu::noinline]] void operator delete[](void* p) noexcept {
    std::free(p);
}

[[gnu::noinline]] void operator delete(void* p, std::size_t) noexcept {
    std::free(p);
}

[[gnu::noinline]] void operator delete[](void* p, std::size_t) noexcept {
    std::free(p);
}

[[gnu::noinline]] void* operator new(std::size_t n, std::align_val_t align) {
    allocate_point();
    void* p;
    if (::posix_memalign(&p, static_cast<std::size_t>(align), n ? n : 1) == 0) {
        return p;
    }
    throw std::bad_alloc();
}

void* operator new[](std::size_t n, std::align_val_t a) {
    return ::operator new(n, a);
}

void* operator new(std::size_t n, std::align_val_t align, const std::nothrow_t&) noexcept {
    try {
        return ::operator new(n, align);
    } catch (...) {
        return nullptr;
    }
}

void* operator new[](std::size_t n, std::align_val_t align, const std::nothrow_t& tag) noexcept {
    return ::operator new(n, align, tag);
}

void operator delete(void* p, std::align_val_t, const std::nothrow_t&) noexcept {
    std::free(p);
}

void operator delete[](void* p, std::align_val_t, const std::nothrow_t&) noexcept {
    std::free(p);
}

[[gnu::noinline]] void operator delete(void* p, std::align_val_t) noexcept {
    std::free(p);
}

[[gnu::noinline]] void operator delete[](void* p, std::align_val_t) noexcept {
    std::free(p);
}

[[gnu::noinline]] void operator delete(void* p, std::size_t, std::align_val_t) noexcept {
    std::free(p);
}

[[gnu::noinline]] void operator delete[](void* p, std::size_t, std::align_val_t) noexcept {
    std::free(p);
}

using namespace vtr::detail::engine;
namespace fs = std::filesystem;

void check(bool good, const char* why) {
    if (!good) {
        throw std::runtime_error(why);
    }
}

template <class F> void rejects(F&& action) {
    try {
        action();
    } catch (const vtr::detail::wire::Error&) {
        return;
    }
    throw std::runtime_error("poisoned writer accepted an operation");
}

void constructor_failures(const fs::path& dir, unsigned workers) {
    auto path = dir / ("constructor-w" + std::to_string(workers) + ".vtr");
    auto filename = path.string();
    for (unsigned point = 0; point < 300; ++point) {
        fs::remove(path);
        bool failed = false;
        std::unique_ptr<Writer> writer;
        fail_after = point;
        try {
            writer = std::make_unique<Writer>(filename, WriterOptions{.workers = workers});
        } catch (const std::bad_alloc&) {
            failed = true;
        }
        fail_after = -1;
        if (failed) {
            if (fs::exists(path)) {
                std::ifstream file(path, std::ios::binary);
                Bytes bytes(std::istreambuf_iterator<char>(file), {});
                const std::array<std::uint8_t, 8> trailer{'V', 'T', 'R', '_', 'E', 'N', 'D', 0};
                check(bytes.size() < 8 ||
                          !std::equal(trailer.begin(), trailer.end(), bytes.end() - 8),
                      "failed constructor published a finalized trace");
            }
        } else {
            writer->close();
            check(!Reader(path).validate().incomplete_file,
                  "successful construction did not finalize");
            std::cout << "constructor/workers=" << workers << ": " << point
                      << " transient allocation failures preserved construction boundary\n";
            return;
        }
    }
    throw std::runtime_error("construction allocation sweep never reached success");
}

void allocation_failures(const fs::path& dir, unsigned workers) {
    auto payload = capture(std::string(4096, 'x'));
    for (std::string action :
         {"owner", "stage", "stage-payload", "finish", "event", "flush", "wave", "declaration",
          "attribute", "owned-finish", "typed-finish", "batch"}) {
        auto path = dir / (action + "-w" + std::to_string(workers) + ".vtr");
        unsigned failures = 0;
        for (unsigned point = 0; point < 300; ++point) {
            bool failed = false;
            {
                Writer writer(path, {.codec = vtr::detail::wire::Codec::stored,
                                     .block_bytes = 1024,
                                     .workers = workers});
                auto ordinary = writer.add_generator(none, "ordinary", "TRANSACTION");
                auto typed = writer.add_generator(none, "typed", "TRANSACTION", 13);
                auto stage = writer.add_generator(none, "stage", "STAGE", 13);
                auto event = writer.add_generator(none, "event", "EVENT", 13);
                auto signal = writer.add_var(none, "variable", {SignalKind::varlen, 1, 2}).second;
                auto scalar = writer.add_var(none, "scalar", {SignalKind::bits, 64, 2}).second;
                auto key = writer.intern("attribute");
                auto value = Value::text("copied text");
                auto seed = writer.begin(ordinary, 0);
                writer.end(seed, 0);
                writer.flush();
                auto owner = writer.begin(
                    (action == "owned-finish" || action == "typed-finish") ? typed : ordinary, 0);
                auto final_payload = payload;
                const std::string typed_value(4096, 'x');
                RecordId pending = 0;
                if (action == "stage-payload") {
                    pending = writer.begin_stage(stage, owner, 0);
                }
                if (action == "finish") {
                    writer.begin_stage(stage, owner, 0, payload);
                }
                if (action == "flush") {
                    writer.event(event, owner, 0, capture(std::string("small")));
                }
                fail_after = point;
                try {
                    if (action == "owner") {
                        writer.begin(typed, 0, payload);
                    }
                    if (action == "stage") {
                        writer.begin_stage(stage, owner, 0, payload);
                    }
                    if (action == "stage-payload") {
                        writer.stage_payload(pending, payload);
                    }
                    if (action == "finish") {
                        writer.end(owner, 1);
                    }
                    if (action == "event") {
                        writer.event(event, owner, 0, payload);
                    }
                    if (action == "flush") {
                        writer.flush();
                    }
                    if (action == "wave") {
                        writer.emit_varlen(signal, payload);
                    }
                    if (action == "declaration") {
                        writer.add_generator(none, "a long dynamically allocated generator name",
                                             "TRANSACTION");
                    }
                    if (action == "attribute") {
                        writer.attribute(owner, key, value);
                    }
                    if (action == "owned-finish") {
                        writer.end(owner, 1, std::move(final_payload));
                    }
                    if (action == "typed-finish") {
                        writer.end_value(owner, 1, typed_value);
                    }
                    if (action == "batch") {
                        const std::array updates{SignalUpdate{scalar, 1}, SignalUpdate{scalar, 2}};
                        writer.emit(updates);
                        writer.flush();
                    }
                } catch (const std::bad_alloc&) {
                    failed = true;
                }
                fail_after = -1;
                if (failed) {
                    ++failures;
                    rejects([&] { writer.close(); });
                    rejects([&] { writer.set_time(2); });
                    writer.seal();
                } else {
                    writer.close();
                }
            }
            Reader reader(path, {.verify_checksums = true});
            auto report = reader.validate();
            check(report.records >= 1 && report.incomplete_file == failed,
                  "allocation failure hid committed prefix or finalized poison");
            if (!failed) {
                break;
            }
            check(point < 299, "allocation sweep never reached success");
        }
        check(failures != 0, "allocation sweep did not exercise operation");
        std::cout << action << "/workers=" << workers << ": " << failures
                  << " allocation failures preserved prefix\n";
    }
}

void allocation_free_recording(const fs::path& dir) {
    const auto path = dir / "allocation-free.vtr";
    vtr::Writer writer(path, {.buffer = {.queue_bytes = 4096}});
    const auto signal = writer.signal({.name = "signal"}, {.width = 32});
    const auto source = writer.transactions<std::array<std::uint64_t, 4>>({.name = "request"});
    const auto logs = writer.logs<std::string>({.name = "message"});
    const auto key = writer.intern("sequence");
    writer.advance_time(1);
    writer.flush();
    constexpr unsigned count = 1000;
    fail_after = 0;
    for (unsigned i = 0; i < count; ++i) {
        writer.advance_time(i + 2);
        writer.emit(signal, i);
        const std::array<vtr::SignalUpdate, 1> updates{{{signal, i}}};
        writer.emit(updates);
        auto record = writer.begin(source, std::array<std::uint64_t, 4>{i, 2, 3, 4});
        writer.attribute(record, key, vtr::Value(std::uint64_t(i)));
        writer.log(logs, std::string("short"), record);
        writer.end(record);
        writer.publish();
    }
    writer.advance_time(count + 2);
    writer.release_producer();
    fail_after = -1;
    writer.close();
    Reader reader(path);
    check(reader.validate().records == count * 2, "allocation-free recording lost records");
    unsigned seen = 0;
    reader.transactions({}, [&](const Transaction& record) {
        const auto value = decode<std::array<std::uint64_t, 4>>(record.payload);
        check(value[0] == seen++ && value[1] == 2 && value[2] == 3 && value[3] == 4,
              "inline payload changed after queue reuse");
        return true;
    });
    check(seen == count, "inline payload records are missing");
}

void allocation_free_large_capture(const fs::path& dir) {
    const auto path = dir / "allocation-free-large.vtr";
    vtr::Writer writer(path);
    using Payload = std::vector<std::string_view>;
    const auto source = writer.transactions<Payload>({.name = "owner"});
    const auto stages = writer.stages<Payload>({.name = "stage"});
    const auto events = writer.events<Payload>({.name = "event"});
    const auto logs = writer.logs<Payload>({.name = "log"});
    std::array<std::string, 4> storage;
    Payload value;
    for (auto& part : storage) {
        part.assign(1024, 'a');
        value.push_back(part);
    }
    writer.advance_time(0);
    constexpr unsigned count = 64;
    fail_after = 0;
    for (unsigned i = 0; i < count; ++i) {
        writer.advance_time(i + 1);
        storage[0][0] = 'A' + i % 26;
        auto owner = writer.begin(source, value);
        auto stage = writer.begin(stages, owner, value);
        writer.payload(owner, value);
        writer.payload(stage, value);
        writer.event(events, owner, value);
        writer.log(logs, value, owner);
        writer.end_at(stage, i + 1, value);
        writer.end_at(owner, i + 1, value, vtr::Status::ok);
        storage[0][0] = '?';
    }
    writer.finish_producer();
    fail_after = -1;
    writer.close();
    Reader reader(path);
    const auto verify = [&](Timestamp time, View payload) {
        const auto decoded = decode<std::vector<std::string>>(payload);
        check(decoded.size() == 4 && decoded[0].size() == 1024 &&
                  decoded[0][0] == 'A' + (time - 1) % 26 && decoded[0][1] == 'a' &&
                  decoded[3] == std::string(1024, 'a'),
              "direct nested capture borrowed mutable storage");
    };
    reader.transactions({}, [&](const Transaction& record) {
        verify(record.begin, record.payload);
        return true;
    });
    reader.records({}, [&](const LocatedRecord& record) {
        verify(record.begin, record.payload);
        return true;
    });
    check(reader.validate().records == count * 4, "direct nested capture lost observations");
}

void failed_direct_capture(const fs::path& dir) {
    for (const auto size : {300u, 4096u}) {
        const auto path = dir / ("failed-direct-" + std::to_string(size) + ".vtr");
        Stream stream(path, {}, {.queue_bytes = 512});
        const auto generator = stream.control([](Writer& backend) {
            return backend.add_generator(none, "message", "LOG", backend.bind<std::string>());
        });
        stream.advance_time(1);
        stream.log(generator, 1, capture(std::string("before")));
        stream.advance_time(2);
        stream.flush();
        const vtr::detail::EncodedPayload broken{size, nullptr,
                                                 [](std::span<std::byte> output, const void*) {
                                                     output[0] = std::byte{7};
                                                     throw std::bad_alloc();
                                                 }};
        bool rejected = false;
        try {
            stream.log(generator, 2, broken);
        } catch (const std::bad_alloc&) {
            rejected = true;
        }
        check(rejected, "direct capture did not invoke its encoder");
        rejected = false;
        try {
            stream.advance_time(3);
        } catch (const std::bad_alloc&) {
            rejected = true;
        }
        check(rejected, "failed direct capture did not stop recording");
        stream.release_producer();
        stream.seal();
        Reader reader(path);
        const auto report = reader.validate();
        check(report.incomplete_file && report.records == 1,
              "partial direct payload was published");
    }
}

void oversized_publication_failure(const fs::path& dir) {
    const auto path = dir / "oversized-publication.vtr";
    Stream stream(path, {}, {.queue_bytes = 256});
    const auto generator = stream.control([](Writer& backend) {
        return backend.add_generator(none, "message", "LOG", backend.bind<std::string>());
    });
    stream.advance_time(1);
    const auto value = capture(std::string(4096, 'x'));
    fail_after = 0;
    bool rejected = false;
    try {
        (void)stream.log(generator, 1, value);
    } catch (const std::bad_alloc&) {
        rejected = true;
    }
    fail_after = -1;
    check(rejected, "oversized capture did not exercise its allocation slow path");
    rejected = false;
    try {
        stream.advance_time(2);
    } catch (const std::bad_alloc&) {
        rejected = true;
    }
    check(rejected, "failed oversized capture did not stop recording");
    stream.release_producer();
    stream.seal();
    check(Reader(path).validate().incomplete_file, "failed oversized capture finalized output");
}

void initial_creation_failure(const fs::path& dir) {
    const auto path = dir / "initial-creation.vtr";
    Stream stream(path);
    const auto generator = stream.control(
        [](Writer& backend) { return backend.add_generator(none, "request", "TRANSACTION"); });
    std::exception_ptr error;
    std::thread worker([&] {
        try {
            bool rejected = false;
            fail_after = 0;
            try {
                (void)stream.begin(generator, 0);
            } catch (const std::bad_alloc&) {
                rejected = true;
            }
            fail_after = -1;
            check(rejected, "initial creation did not exercise backend startup failure");
            rejected = false;
            try {
                (void)stream.begin(generator, 1);
            } catch (const std::bad_alloc&) {
                rejected = true;
            }
            check(rejected, "creation failure abandoned an ID without stopping recording");
            stream.seal();
        } catch (...) {
            error = std::current_exception();
        }
    });
    worker.join();
    if (error) {
        std::rethrow_exception(error);
    }
    check(Reader(path).validate().incomplete_file,
          "startup failure finalized an incomplete recording");
}

void write_failure(const fs::path& dir, unsigned workers) {
    auto path = dir / ("write-error-w" + std::to_string(workers) + ".vtr");
    Writer writer(
        path, {.codec = vtr::detail::wire::Codec::stored, .block_bytes = 1024, .workers = workers});
    auto ordinary = writer.add_generator(none, "ordinary", "TRANSACTION");
    auto event = writer.add_generator(none, "event", "EVENT", 13);
    auto seed = writer.begin(ordinary, 0);
    writer.end(seed, 0);
    writer.flush();
    auto owner = writer.begin(ordinary, 0);
    auto payload = capture(std::string(4096, 'x'));
    struct rlimit saved;
    check(::getrlimit(RLIMIT_FSIZE, &saved) == 0, "cannot read file limit");
    auto previous_handler = std::signal(SIGXFSZ, SIG_IGN);
    struct rlimit limited = saved;
    limited.rlim_cur = fs::file_size(path) + 32;
    check(::setrlimit(RLIMIT_FSIZE, &limited) == 0, "cannot set file limit");
    bool failed = false;
    try {
        writer.event(event, owner, 1, payload);
        writer.close();
    } catch (const vtr::detail::wire::Error& error) {
        failed = error.code == 3;
    }
    check(::setrlimit(RLIMIT_FSIZE, &saved) == 0, "cannot restore file limit");
    std::signal(SIGXFSZ, previous_handler);
    check(failed, "file-size limit did not produce write failure");
    rejects([&] { writer.close(); });
    rejects([&] { writer.set_time(3); });
    writer.seal();
    Reader reader(path, {.verify_checksums = true});
    auto report = reader.validate();
    check(report.incomplete_file && report.records == 1 && !reader.fingerprint(),
          "I/O failure did not preserve exactly the committed prefix");
}

int main(int argc, char** argv) {
    try {
        if (argc != 2) {
            throw std::runtime_error("usage: failure_test DIRECTORY");
        }
        fs::create_directories(argv[1]);
        allocation_free_recording(argv[1]);
        allocation_free_large_capture(argv[1]);
        failed_direct_capture(argv[1]);
        oversized_publication_failure(argv[1]);
        initial_creation_failure(argv[1]);
        for (unsigned workers : {0, 2}) {
            constructor_failures(argv[1], workers);
            allocation_failures(argv[1], workers);
            write_failure(argv[1], workers);
        }
    } catch (const std::exception& error) {
        fail_after = -1;
        std::cerr << error.what() << '\n';
        return 1;
    }
}
