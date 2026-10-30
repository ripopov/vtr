#include "../examples/logging.h"

#include <clocale>
#include <cmath>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <limits>
#include <random>

namespace vt = vtr;

void check(bool condition, const char* message) {
    if (!condition)
        throw std::runtime_error(message);
}

template <class... Args> std::string reference(const std::string& format, Args... args) {
    const auto size = std::snprintf(nullptr, 0, format.c_str(), args..., 0);
    check(size >= 0, "reference formatting failed");
    std::string result(static_cast<std::size_t>(size) + 1, '\0');
    check(std::snprintf(result.data(), result.size(), format.c_str(), args..., 0) == size,
          "reference size changed");
    result.resize(static_cast<std::size_t>(size));
    return result;
}

class Cases {
    struct Expected {
        vt::RecordId id;
        std::string format, message;
    };

    std::filesystem::path path_;
    vt::Writer writer_;
    vt::Logger logger_;
    std::vector<Expected> expected_;

  public:
    Cases(std::filesystem::path path, vt::ExecutionMode mode)
        : path_(std::move(path)), writer_(path_, {.mode = mode}),
          logger_(writer_.logger({.name = "printf"})) {}

    template <class... Args>
    void add(std::string format, std::string expected, const Args&... args) {
        const auto arguments = logging::arguments(args...);
        const auto site = vt::logging::declare<std::remove_cv_t<decltype(arguments)>>(
            writer_, {.format = format, .severity = 1, .style = vt::logging::FormatStyle::printf});
        const auto id = writer_.log_at(logger_, site, expected_.size(), arguments);
        expected_.push_back({id, std::move(format), std::move(expected)});
        writer_.advance_time(expected_.size());
    }

    template <class... Args> void compare(const std::string& format, const Args&... args) {
        add(format, reference(format, args...), args...);
    }

    void verify() {
        writer_.close();
        vt::Reader reader(path_);
        check(reader.validate().records == expected_.size(), "record count changed");
        std::ofstream oracle(path_.string() + ".expected");
        for (const auto& expected : expected_) {
            oracle << expected.id.value << " ";
            for (unsigned char byte : expected.message)
                oracle << std::hex << std::setw(2) << std::setfill('0') << unsigned(byte);
            oracle << std::dec << "\n";
            const auto record = reader.lookup(expected.id);
            check(record.has_value(), "record missing");
            const auto metadata = vt::logging::metadata(reader, record->datatype());
            check(metadata && metadata->style == vt::logging::FormatStyle::printf &&
                      metadata->format == expected.format,
                  "printf metadata changed");
            const auto actual = vt::logging::format_log(reader, *record);
            if (actual != expected.message)
                throw std::runtime_error("format '" + expected.format + "': expected '" +
                                         expected.message + "', got '" + actual + "'");
            check(actual ==
                      reader.format_printf_arguments(expected.format, record->payload()["value"]),
                  "direct printf renderer differs");
        }
        std::cout << "verified " << expected_.size() << " printf cases in " << path_ << '\n';
    }
};

void integers(Cases& cases) {
    for (const auto conversion : {'d', 'i'}) {
        for (const auto flags : {"", "+", " ", "-", "0", "+0", "-+"}) {
            for (const auto tail : {"", "12", "12.0", "12.3", ".0", ".20"}) {
                const auto format = "%" + std::string(flags) + tail + conversion;
                for (const int value : {INT_MIN, -12345, -1, 0, 1, 12345, INT_MAX})
                    cases.compare(format, value);
            }
        }
    }
    for (const auto conversion : {'u', 'o', 'x', 'X'}) {
        for (const auto flags : {"", "-", "0", "#", "#0", "-#"}) {
            for (const auto tail : {"", "12", "12.0", "12.3", ".0", ".20"}) {
                const auto format = "%" + std::string(flags) + tail + conversion;
                for (const unsigned value : {0u, 1u, 42u, 0x80000000u, UINT_MAX})
                    cases.compare(format, value);
            }
        }
    }
    for (const int value : {INT_MIN, -257, -129, -1, 0, 127, 128, 256, INT_MAX}) {
        cases.compare("%hhd %hd", value, value);
        cases.compare("%hhu %hu", static_cast<unsigned>(value), static_cast<unsigned>(value));
    }
    cases.compare("%ld %lu %#lx", LONG_MIN, ULONG_MAX, ULONG_MAX);
    cases.compare("%lld %llu %#llX", LLONG_MIN, ULLONG_MAX, ULLONG_MAX);
    cases.compare("%jd %ju", INTMAX_MIN, UINTMAX_MAX);
    cases.compare("%zd %zu", std::numeric_limits<std::make_signed_t<std::size_t>>::min(), SIZE_MAX);
    cases.compare("%td %tu", PTRDIFF_MIN,
                  std::numeric_limits<std::make_unsigned_t<std::ptrdiff_t>>::max());
    cases.add("%d %u %#x", "18446744073709551615 18446744073709551615 0xffffffffffffffff",
              UINT64_MAX, UINT64_MAX, UINT64_MAX);
    cases.add("%x %u", "ffffffff 4294967295", -1, -1);
    cases.add("%d %d %d", "-128 -32768 -9223372036854775808", std::int8_t{-128},
              std::int16_t{-32768}, INT64_MIN);
    cases.add("%b %#B %08b %#08b %.0b %8.6b", "101 0B101 00000101 0b000101    000101", 5u, 5u, 5u,
              5u, 0u, 5u);
    cases.add("%#-8B", "0B101   ", 5u);
}

void floating(Cases& cases) {
    const double values[] = {0.0,
                             -0.0,
                             1.0,
                             -1.25,
                             0.00001,
                             1234567.75,
                             std::numeric_limits<double>::denorm_min(),
                             std::numeric_limits<double>::min(),
                             std::numeric_limits<double>::max(),
                             std::numeric_limits<double>::infinity(),
                             -std::numeric_limits<double>::infinity(),
                             std::numeric_limits<double>::quiet_NaN()};
    for (const auto conversion : {'f', 'F', 'e', 'E', 'g', 'G', 'a', 'A'}) {
        for (const auto flags : {"", "+", " ", "#", "0", "-", "+#0"}) {
            for (const auto tail : {"", "18", "18.0", "18.4", ".17"}) {
                const auto format = "%" + std::string(flags) + tail + conversion;
                for (const auto value : values)
                    cases.compare(format, value);
            }
        }
    }
    cases.add("%Lf %La", reference("%Lf %La", 1.25L, 1.25L), 1.25, 1.25);
    cases.compare("%lf", 1.25);
    cases.compare("%.9g", float{1.2345678f});
    std::mt19937_64 random(0x7072696e7466);
    for (unsigned i = 0; i < 256; ++i) {
        const auto value = std::bit_cast<double>(random());
        if (std::isfinite(value))
            cases.compare("%+.17g %#.13a", value, value);
    }
}

void dynamic_and_positional(Cases& cases) {
    for (const auto width : {-20, 0, 20}) {
        for (const auto precision : {INT_MIN, -1, 0, 3, 12}) {
            const std::string format = "[%*.*d] [%*.*f] [%*.*s]";
            const auto normalized = precision < 0 ? -1 : precision;
            cases.add(format,
                      reference(format, width, normalized, -42, width, normalized, 1.25, width,
                                normalized, "hello"),
                      width, precision, -42, width, precision, 1.25, width, precision, "hello");
        }
    }
    cases.compare("[%3$*1$.*2$f] %4$s %3$g", 12, 3, 1.25, "done");
    cases.compare("%2$d %1$d %2$#x", 11, 42);
    cases.compare("%1$*1$d", 4);
    cases.compare("%2$.*1$s", 3, "hello");
    cases.add("%2$d", "42", 11, 42);
    cases.add("%*d", std::string(65535, ' ') + "1", 65536, 1);
}

void text_and_addresses(Cases& cases) {
    cases.compare("plain %% braces {} %%");
    cases.compare("[%s] [%12.3s] [%-12s]", "hello", "hello", "hello");
    cases.compare("%c/%3c/%-3c", 'A', 0, 255);
    cases.add("%s", "before", std::string("before"));
    cases.add("%s", "left", std::string("left\0right", 10));
    for (const auto address : {std::uintptr_t{0}, std::uintptr_t{0x1234}, UINTPTR_MAX}) {
        cases.add("[%p] [%-20p]",
                  reference("[%p] [%-20p]", reinterpret_cast<void*>(address),
                            reinterpret_cast<void*>(address)),
                  vt::Address{address}, vt::Address{address});
    }
    const auto* locale = std::setlocale(LC_ALL, "C.UTF-8");
    if (!locale)
        locale = std::setlocale(LC_ALL, "en_US.UTF-8");
    check(locale, "UTF-8 locale unavailable");
    cases.add("%lc %ls %8.3ls",
              reference("%lc %ls %8.3ls", L'\u03bb', L"\u03bbhello", L"\u03bbhello"),
              std::uint32_t{0x3bb}, std::string("\xce\xbbhello"), std::string("\xce\xbbhello"));
    cases.compare("%'d %'.2f", 1234567, 12345.5);
}

void malformed(Cases& cases) {
    for (const auto format :
         {"%", "%q", "%Ld", "%hs", "%llf", "%lp", "%0$d", "%184467440737095516160$d",
          "%999999999999999999999d", "%65537d", "%.65537f", "%1$d %d", "%d %2$d", "%1$*d"})
        cases.add(format, "{?}", 1, 2);
    cases.add("missing=%d", "missing={?}");
    cases.add("wrong=%d", "wrong={?}", "hello");
    cases.add("wrong=%s", "wrong={?}", 42);
    cases.add("wrong=%f", "wrong={?}", 42);
    cases.add("wrong=%c", "wrong={?}", "hello");
    cases.add("wrong=%p", "wrong={?}", 1.0);
    cases.add("%*d", "{?}", INT_MIN, 1);
    cases.add("%*d", "{?}", 65537, 1);
    cases.add("%.*f", "{?}", 65537, 1.0);
    cases.add("%*d", "{?}", "width", 1);
    cases.add("%n %d", "{?} 42", vt::Address{0x1234}, 42);
    cases.add("%ln", "{?}", vt::Address{0});
    cases.add("%.1c", "{?}", 'a');
    cases.add("%+s", "{?}", "text");
    cases.add("%.1p", "{?}", vt::Address{0x1234});
    cases.add("%lc", "{?}", std::uint32_t{0xd800});
    cases.add("%s", "{?}", std::vector<int>{1, 2});
}

void reader_limits(const std::filesystem::path& directory) {
    const auto path = directory / "limits.vtr";
    vt::Writer writer(path);
    const auto logger = writer.logger({.name = "limits"});
    const auto payload = logging::arguments(1, 2);
    const auto site = vt::logging::declare<std::remove_cv_t<decltype(payload)>>(
        writer, {.format = "%40d%40d", .style = vt::logging::FormatStyle::printf});
    const auto id = writer.log(logger, site, payload);
    writer.close();
    vt::Reader reader(path, {.max_payload_bytes = 64});
    const auto record = reader.lookup(id);
    bool rejected = false;
    try {
        (void)vt::logging::format_log(reader, *record);
    } catch (const vt::Error& error) {
        rejected = error.code() == vt::ErrorCode::resource_limit;
    }
    check(rejected, "printf output bypasses reader allocation limit");
    vt::Reader other(path);
    rejected = false;
    const auto other_path = directory / "other.vtr";
    vt::Writer(other_path).close();
    vt::Reader different(other_path);
    try {
        (void)different.format_printf_arguments("%d", other.lookup(id)->payload()["value"]);
    } catch (const vt::Error& error) {
        rejected = error.code() == vt::ErrorCode::wrong_recording;
    }
    check(rejected, "printf accepts foreign payload");
    const auto arguments = other.lookup(id)->payload()["value"];
    check(other.format_printf_arguments(std::string(2 * 65535, '%') + "x%d", arguments) == "{?}",
          "printf token budget exceeded at the final conversion");
    std::mt19937 random(0x7072696e);
    const std::string alphabet = "%$0123456789.*+- #hljztLdiuoxXfFeEgGaAcspn";
    for (unsigned sample = 0; sample < 5000; ++sample) {
        std::string format;
        for (unsigned length = random() % 60; length; --length)
            format += alphabet[random() % alphabet.size()];
        (void)other.format_printf_arguments(format, arguments);
    }
}

void metadata_styles(const std::filesystem::path& directory) {
    const auto path = directory / "styles.vtr";
    vt::Writer writer(path);
    const auto logger = writer.logger({.name = "styles"});
    const auto brace = vt::logging::declare<int>(writer, {.format = "%d"});
    const auto printf = vt::logging::declare<int>(
        writer, {.format = "%d", .style = vt::logging::FormatStyle::printf});
    const auto brace_id = writer.log(logger, brace, 42);
    const auto printf_id = writer.log(logger, printf, 42);
    check(brace.id() != printf.id(), "format styles share a datatype");
    const auto raw = vt::logging::declare_raw(
        writer, {.format = "%d", .style = vt::logging::FormatStyle::printf}, writer.bind<int>());
    const std::array<std::byte, 4> bytes{std::byte{42}};
    const auto raw_id = writer.log_raw(logger, raw, 0, bytes);
    std::vector<vt::RecordId> invalid;
    for (const auto& style : {vt::Value(std::string("unknown")), vt::Value(std::uint64_t{1})}) {
        auto options = vt::logging::datatype_options(writer, {.format = "%d"});
        options.attributes.push_back({writer.intern("log.format_style"), style});
        const auto type = writer.datatype<int>(std::move(options));
        invalid.push_back(writer.log(logger, type, 42));
    }
    bool rejected = false;
    try {
        (void)vt::logging::declare<int>(
            writer, {.format = "%d", .style = static_cast<vt::logging::FormatStyle>(99)});
    } catch (const vt::Error& error) {
        rejected = error.code() == vt::ErrorCode::invalid_argument;
    }
    check(rejected, "writer accepts unknown format style");
    writer.close();
    vt::Reader reader(path);
    check(reader.validate().records == 5, "metadata tests lost records");
    check(vt::logging::format_log(reader, *reader.lookup(brace_id)) == "%d" &&
              vt::logging::format_log(reader, *reader.lookup(printf_id)) == "42" &&
              vt::logging::format_log(reader, *reader.lookup(raw_id)) == "42",
          "style dispatch or raw printf capture changed");
    for (const auto id : invalid) {
        rejected = false;
        try {
            (void)vt::logging::format_log(reader, *reader.lookup(id));
        } catch (const vt::Error& error) {
            rejected = error.code() == vt::ErrorCode::invalid_argument;
        }
        check(rejected, "reader silently accepts unknown format style");
    }
}

int main(int argc, char** argv) {
    try {
        if (argc == 3 && std::string_view(argv[1]) == "--read") {
            vt::Reader reader(argv[2]);
            (void)reader.validate();
            std::ifstream oracle(std::string(argv[2]) + ".expected");
            check(bool(oracle), "missing expected messages");
            std::string line;
            while (std::getline(oracle, line)) {
                const auto split = line.find(' ');
                const auto id = std::stoull(line.substr(0, split));
                std::string expected;
                for (auto pos = split + 1; pos < line.size(); pos += 2)
                    expected += static_cast<char>(std::stoul(line.substr(pos, 2), nullptr, 16));
                check(vt::logging::format_log(reader, *reader.lookup(vt::RecordId{id})) == expected,
                      "Rust printf trace renders differently in C++");
            }
            return 0;
        }
        check(argc == 2, "usage: printf_test DIRECTORY");
        const std::filesystem::path directory(argv[1]);
        std::filesystem::create_directories(directory);
        for (const auto mode : {vt::ExecutionMode::inline_, vt::ExecutionMode::threaded}) {
            Cases cases(directory /
                            (mode == vt::ExecutionMode::inline_ ? "inline.vtr" : "threaded.vtr"),
                        mode);
            integers(cases);
            floating(cases);
            dynamic_and_positional(cases);
            text_and_addresses(cases);
            malformed(cases);
            cases.verify();
            Cases sample(directory / (mode == vt::ExecutionMode::inline_ ? "sample-inline.vtr" : "sample-threaded.vtr"), mode);
            sample.add("pc=%#018x value=%+.2f label=%.3s", "pc=0xffffffffffffffff value=-1.25 label=abc", UINT64_MAX, -1.25, std::string("abcdef"));
            sample.add("[%3$*1$.*2$f]", "[    1.250]", 9, 3, 1.25);
            sample.add("%c", std::string(1, char(255)), 255);
            sample.verify();
        }
        reader_limits(directory);
        metadata_styles(directory);
        return 0;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
