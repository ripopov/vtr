#pragma once

// Static callsites share argument encoders and writer-bound declarations.
#include "vtr_logging.h"

#include <array>
#include <atomic>
#include <format>
#include <limits>
#include <memory>
#include <source_location>
#include <string_view>
#include <tuple>
#include <type_traits>
#include <utility>
#include <vector>

namespace logging {

enum class Severity : std::uint64_t { info = 2, warning = 3, off = 6 };

struct Metadata {
    std::string_view format;
    Severity severity;
    std::source_location location;
};

constexpr std::size_t decimal_digits(std::size_t value) {
    std::size_t digits = 1;
    while (value >= 10) {
        value /= 10;
        ++digits;
    }
    return digits;
}

template <std::size_t Index, class T> struct Argument {
    T value;

    static constexpr auto name = [] {
        std::array<char, 3 + decimal_digits(Index)> text{'a', 'r', 'g'};
        auto value = Index;
        for (auto position = text.size(); position > 3; value /= 10) {
            text[--position] = static_cast<char>('0' + value % 10);
        }
        return text;
    }();
};

template <class Indices, class... Args> struct Arguments;

template <std::size_t... Indices, class... Args>
struct Arguments<std::index_sequence<Indices...>, Args...> : Argument<Indices, Args>... {
    static consteval bool valid_format(std::format_string<Args...>) {
        return true;
    }
};

template <class T> auto argument(const T& value) {
    if constexpr (std::is_convertible_v<const T&, std::string_view>) {
        return std::string_view(value);
    } else {
        return value;
    }
}

template <class... Args> auto arguments(const Args&... args) {
    return Arguments<std::index_sequence_for<Args...>, decltype(argument(args))...>{
        {argument(args)}...};
}

} // namespace logging

template <std::size_t... Indices, class... Args>
struct vtr::Schema<::logging::Arguments<std::index_sequence<Indices...>, Args...>> {
    static constexpr std::string_view name = "LogArguments";
    static constexpr auto fields = std::tuple{
        vtr::field(std::string_view{::logging::Argument<Indices, Args>::name.data(),
                                    ::logging::Argument<Indices, Args>::name.size()},
                   &::logging::Argument<Indices, Args>::value)...,
    };
};

namespace logging {

inline std::atomic<std::size_t> next_callsite_slot{0};

template <class Payload> class Callsite {
  public:
    constexpr explicit Callsite(const Metadata& metadata) : metadata_(metadata) {}

    Callsite(const Callsite&) = delete;
    Callsite& operator=(const Callsite&) = delete;

  private:
    friend class Logger;
    static constexpr auto unbound = std::numeric_limits<std::size_t>::max();

    std::size_t claim_slot() const noexcept {
        // The slot publishes no data: metadata is static and source tables belong to each logger.
        auto slot = slot_.load(std::memory_order_relaxed);
        if (slot == unbound) {
            const auto candidate = next_callsite_slot.fetch_add(1, std::memory_order_relaxed);
            if (slot_.compare_exchange_strong(slot, candidate, std::memory_order_relaxed)) {
                slot = candidate;
            }
        }
        return slot;
    }

    const Metadata& metadata_;
    mutable std::atomic<std::size_t> slot_{unbound};
};

// One logger belongs to one writer and logical producer; serialize its calls.
class Logger {
  public:
    explicit Logger(vtr::Writer& writer, std::string_view name = "logger")
        : writer_(writer), logger_(writer.logger({.name = name})) {}

    Logger(vtr::Writer& writer, vtr::Logger logger) : writer_(writer), logger_(logger) {}

    [[nodiscard]] vtr::GeneratorId id() const noexcept {
        return logger_.id();
    }

    void set_level(Severity level) noexcept {
        level_ = level;
    }

    [[nodiscard]] bool enabled(Severity severity) const noexcept {
        return severity >= level_;
    }

    template <class Payload> const auto& prepare(const Callsite<Payload>& site) {
        auto slot = site.slot_.load(std::memory_order_relaxed);
        if (slot >= sources_.size() || !sources_[slot]) [[unlikely]] {
            slot = bind(site);
        }
        // Slots belong to immutable typed callsites, so a binding's type cannot change.
        return static_cast<const Binding<Payload>&>(*sources_[slot]).source;
    }

    template <class Payload> void write(const Callsite<Payload>& site, const Payload& payload) {
        writer_.log(logger_, prepare(site), payload);
    }

  private:
    struct BindingBase {
        virtual ~BindingBase() = default;
    };

    template <class Payload> struct Binding : BindingBase {
        explicit Binding(vtr::PayloadType<Payload> source) : source(source) {}

        vtr::PayloadType<Payload> source;
    };

    template <class Payload> [[gnu::noinline]] std::size_t bind(const Callsite<Payload>& site) {
        const auto slot = site.claim_slot();
        const auto& metadata = site.metadata_;
        if (slot >= sources_.size()) {
            sources_.resize(slot + 1);
        }
        auto source = vtr::logging::declare<Payload>(
            writer_,
            {.format = metadata.format,
             .severity = static_cast<std::uint64_t>(metadata.severity),
             .file = metadata.location.file_name(),
             .line = metadata.location.line(),
             .function = metadata.location.function_name()},
            &site);
        sources_[slot] = std::make_unique<Binding<Payload>>(source);
        return slot;
    }

    vtr::Writer& writer_;
    vtr::Logger logger_;
    Severity level_ = Severity::info;
    // Writer-bound handles stay here; static callsites contain no recording state.
    std::vector<std::unique_ptr<BindingBase>> sources_;
};

} // namespace logging

#define LOG_AT(logger, severity, fmt, ...)                                                         \
    do {                                                                                           \
        auto& vtr_log_logger = (logger);                                                           \
        if (vtr_log_logger.enabled(severity)) {                                                    \
            using vtr_log_payload = decltype(::logging::arguments(__VA_ARGS__));                   \
            static constexpr ::logging::Metadata vtr_log_metadata{                                 \
                fmt, severity, std::source_location::current()};                                   \
            static_assert(vtr_log_payload::valid_format(vtr_log_metadata.format));                 \
            static constinit const ::logging::Callsite<vtr_log_payload> vtr_log_callsite{          \
                vtr_log_metadata};                                                                 \
            vtr_log_logger.write(vtr_log_callsite, ::logging::arguments(__VA_ARGS__));             \
        }                                                                                          \
    } while (false)

#define LOG_INFO(logger, format, ...)                                                              \
    LOG_AT(logger, ::logging::Severity::info, format __VA_OPT__(, ) __VA_ARGS__)
#define LOG_WARNING(logger, format, ...)                                                           \
    LOG_AT(logger, ::logging::Severity::warning, format __VA_OPT__(, ) __VA_ARGS__)
