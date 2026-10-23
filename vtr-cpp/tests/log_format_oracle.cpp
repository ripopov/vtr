#include <bit>
#include <climits>
#include <cmath>
#include <concepts>
#include <cstdint>
#include <format>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <limits>
#include <locale>
#include <sstream>
#include <string>
#include <vector>

namespace {
std::string hex(std::string_view bytes) {
    constexpr std::string_view digits = "0123456789abcdef";
    std::string result;
    for (unsigned char value : bytes) {
        result += digits[value >> 4];
        result += digits[value & 15];
    }
    return result;
}

template <class T>
void samples(std::ostream& out, std::string_view type, const std::vector<T>& values,
             const std::vector<std::string>& specs, bool& first,
             const std::locale& locale = std::locale::classic(), std::string_view metadata = {}) {
    for (T value : values) {
        std::uint64_t bits;
        if constexpr (std::same_as<T, float>)
            bits = std::bit_cast<std::uint32_t>(value);
        else if constexpr (std::same_as<T, double>)
            bits = std::bit_cast<std::uint64_t>(value);
        else if constexpr (std::same_as<T, std::string>)
            bits = 0;
        else
            bits = static_cast<std::uint64_t>(value);
        for (const auto& spec : specs) {
            std::string expected;
            try {
                bool fixed_zero = false;
                if constexpr (std::floating_point<T>) {
                    if (std::isfinite(value) && (spec == "#.0Lf" || spec == "#.0f")) {
                        // Independently round before using a nonzero precision (PR120967).
                        auto rounded = std::nearbyint(value);
                        expected = std::vformat(locale, spec == "#.0Lf" ? "{:.1Lf}" : "{:.1f}",
                                                std::make_format_args(rounded));
                        expected.pop_back();
                        fixed_zero = true;
                    }
                }
                if (!fixed_zero)
                    expected =
                        std::vformat(locale, "{:" + spec + "}", std::make_format_args(value));
            } catch (const std::format_error&) {
                expected = "{?}";
            }
            if (!first)
                out << ",\n";
            first = false;
            out << "  {\"type\": " << std::quoted(std::string(type))
                << ", \"bits\": " << std::quoted(std::to_string(bits))
                << ", \"spec\": " << std::quoted(spec)
                << ", \"expected_hex\": " << std::quoted(hex(expected));
            if constexpr (std::same_as<T, std::string>)
                out << ", \"text_hex\": " << std::quoted(hex(value));
            if (!metadata.empty())
                out << ", \"locale\": " << metadata;
            out << '}';
        }
    }
}

struct Punctuation : std::numpunct<char> {
    std::string groups, yes, no;
    char point, separator;

    Punctuation(std::string groups, char point, char separator, std::string yes, std::string no)
        : groups(std::move(groups)), yes(std::move(yes)), no(std::move(no)), point(point),
          separator(separator) {}

    std::string do_grouping() const override {
        return groups;
    }

    char do_decimal_point() const override {
        return point;
    }

    char do_thousands_sep() const override {
        return separator;
    }

    std::string do_truename() const override {
        return yes;
    }

    std::string do_falsename() const override {
        return no;
    }
};

void localized_samples(std::ostream& out, bool& first, std::string grouping, char point = ',',
                       char separator = '_', std::string yes = "yes", std::string no = "no") {
    const std::locale locale(std::locale::classic(),
                             new Punctuation(grouping, point, separator, yes, no));
    std::ostringstream metadata;
    metadata << "{\"grouping_hex\": " << std::quoted(hex(grouping))
             << ", \"point_hex\": " << std::quoted(hex(std::string(1, point)))
             << ", \"separator_hex\": " << std::quoted(hex(std::string(1, separator)))
             << ", \"true_hex\": " << std::quoted(hex(yes))
             << ", \"false_hex\": " << std::quoted(hex(no)) << '}';
    const auto description = metadata.str();
    const std::vector<std::string> integers{"",      "L",      "Ld",     "Lx",      "LX",  "Lo",
                                            "Lb",    "LB",     "#Lx",    "#Lo",     "#Lb", "+020Ld",
                                            "020Ld", ">020Ld", "*^24Ld", "界>24Ld", "Lc",  "8Lc"};
    samples<std::int64_t>(out, "i64", {0, 1, -123456789, INT64_MIN}, integers, first, locale,
                          description);
    samples<std::uint64_t>(out, "u64", {0, 65535, UINT64_MAX}, integers, first, locale,
                           description);
    samples<bool>(out, "bool", {false, true},
                  {"", "s", "L", "Ls", "12Ls", "*^12L", "界>12Ls", "08Ld", "#Lb"}, first, locale,
                  description);
    const std::vector<std::string> floats{"",          "L",     ".2Lf", "+020.2Lf", "*^24.2Lf",
                                          "界>24.2Lf", "#.0Lf", ".3Le", ".3LE",     ".8Lg",
                                          ".8LG",      "La",    ".3LA"};
    samples<double>(out, "f64",
                    {0., -0., 123456789.5, -1234.25, 1.234567e-10,
                     std::numeric_limits<double>::max(), std::numeric_limits<double>::infinity(),
                     -std::numeric_limits<double>::quiet_NaN()},
                    floats, first, locale, description);
    samples<float>(out, "f32", {1234567.5f, -0.125f}, floats, first, locale, description);
}

std::string oracle() {
    std::ostringstream out;
    out << "[\n";
    bool first = true;
    const std::vector<std::string> floats{
        "",     ".0",  ".3",  ".17", "g",    "G",    "e",      "E",   "f",    "F",
        "a",    "A",   "#",   "#g",  "#.0g", "#.4g", ".0e",    ".2E", ".0f",  ".2F",
        ".0g",  ".3G", ".0a", ".2a", ".13a", "+.2f", " 12.2f", "020", "020E", "*^20.3g",
        ">020", "d",   ".f",  "z",   "L",    "L.2f", ".2Lf"};
    samples<double>(out, "f64",
                    {0.,
                     -0.,
                     0.1,
                     -2.5,
                     1e-4,
                     1e-5,
                     1e6,
                     1.234567,
                     999.95,
                     1.5,
                     1.99999999999999,
                     std::bit_cast<double>(3ull),
                     std::bit_cast<double>(0xfffffffffffffull),
                     std::numeric_limits<double>::max(),
                     std::numeric_limits<double>::min(),
                     std::numeric_limits<double>::denorm_min(),
                     std::numeric_limits<double>::infinity(),
                     -std::numeric_limits<double>::infinity(),
                     std::numeric_limits<double>::quiet_NaN(),
                     -std::numeric_limits<double>::quiet_NaN()},
                    floats, first);
    samples<float>(out, "f32",
                   {0.f, -0.f, 0.1f, 1.234567f, std::numeric_limits<float>::max(),
                    std::numeric_limits<float>::min(), std::numeric_limits<float>::denorm_min(),
                    -std::numeric_limits<float>::quiet_NaN()},
                   floats, first);
    const std::vector<std::string> integers{
        "",    "d",     "x",  "X",  "o",  "b",  "B",  "#X",   "#o",   "#b",  "#B", "+08d",
        " 8d", "*^12x", ".2", "f",  "s",  "c",  "5c", "*^5c", "+c",   "-c",  "#c", "0c",
        ".2c", "Lc",    "L",  "Ld", "Lx", "-s", "?",  "05c",  "+05c", "#05c"};
    samples<std::int64_t>(
        out, "i64", {-128, -1, 0, 1, 65, 127, 128, 255, std::numeric_limits<std::int64_t>::min()},
        integers, first);
    samples<std::uint64_t>(out, "u64", {0, 1, 255, std::numeric_limits<std::uint64_t>::max()},
                           integers, first);
    // vector<bool> has proxy references; construct concrete bool values for format_args.
    samples<bool>(out, "bool", {false, true}, integers, first);
    samples<std::string>(out, "text",
                         {"", "plain", "e\xcc\x81x", "\xcc\x81", "界a", "🦀a", "👩‍💻a",
                          "🇺🇸a", "\r\n", "a\tb\n\"\\", "🏳️‍🌈a", "가a",
                          "\xe2\x98\x80\xef\xb8\x8f"},
                         {"",     "s",  "5",  ">5", "*^6", "界>6", ".0", ".1",  ".2",  ".3",
                          "8.2s", "-s", "+s", "#s", "05s", "Ls",   "?",  ".3?", "{<5", "0<5"},
                         first);
    for (const auto& grouping : std::vector<std::string>{
             "", "\3", "\3\2", std::string("\3\0", 2), std::string{3, CHAR_MAX}, std::string(1, 0),
             std::string(1, CHAR_MAX), "\1", "\4\3\2", std::string{3, static_cast<char>(0xff)}})
        localized_samples(out, first, grouping);
    localized_samples(out, first, "\3", static_cast<char>(0xff), static_cast<char>(0xfe),
                      std::string("ye\xffs"), std::string("n\0o", 3));
    localized_samples(out, first, "\3", ',', '_', "はい", "いいえ");
    localized_samples(out, first, "\3", ',', '_', "界\xff界", "");
    out << "\n]\n";
    return out.str();
}
} // namespace

int main(int argc, char** argv) {
    const auto generated = oracle();
    if (argc == 3 && std::string_view(argv[1]) == "--check") {
        std::ifstream input(argv[2], std::ios::binary);
        std::string saved{std::istreambuf_iterator<char>(input), {}};
        if (saved != generated) {
            std::cerr << "C++ scalar formatting differs from the Rust fixture\n";
            return 1;
        }
    } else if (argc == 2) {
        std::ofstream(argv[1], std::ios::binary) << generated;
    } else
        return 2;
}
