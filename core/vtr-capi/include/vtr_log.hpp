/*
 * vtr_log.hpp - header-only C++17 logging front end over the VTR C API.
 *
 *     vtr::LogStream log(writer, scope_node, "log");
 *     VTR_LOG(log, vtr::Severity::Info, sim_time, "AXI write addr={:#x} len={}", addr, len);
 *
 * The first execution of a VTR_LOG statement registers its call site (format
 * string, severity, argument types, file, line, function) as a generator of
 * the LOG stream; every execution then stores only the timestamp and the raw
 * argument values (a few bytes per message before compression). Formatting
 * happens in the reader (`vtr log file.vtr`, `vtr_log_rec_format`, or
 * vtr::format_log below).
 *
 * Placeholders follow the std::format / Rust subset documented in
 * docs/LOGGING.md: `{}`, `{1}`, `{:x}`, `{:#010x}`, `{:>8}`, `{:.3}`, `{{`.
 * The number of placeholders must equal the number of arguments; this is
 * checked at compile time.
 *
 * Argument mapping: bool -> BOOL; signed integers and enums -> I64; unsigned
 * -> U64; float/double -> F64; const char*, std::string, std::string_view ->
 * TEXT (deduplicated per block); vtr::interned{id} -> STR; vtr::bytes -> BYTES;
 * vtr::pointer / T* -> POINTER; vtr::timestamp -> TIME.
 *
 * Neither the writer nor a LogStream is thread-safe: log from the thread that
 * owns the writer (a simulator's main thread).
 */
#ifndef VTR_LOG_HPP
#define VTR_LOG_HPP

#include "vtr.h"

#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>

namespace vtr {

enum class Severity : uint8_t { Trace = 0, Debug = 1, Info = 2, Warn = 3, Error = 4, Fatal = 5 };

inline const char *severity_name(uint8_t s) {
    static const char *const names[] = {"trace", "debug", "info", "warn", "error", "fatal"};
    return s < 6 ? names[s] : "other";
}

/* Argument wrappers selecting a non-default type tag. */
struct interned { uint32_t id; };                    /* VTR_VAL_STR: a string id from vtr_writer_intern */
struct bytes { const uint8_t *data; size_t len; };   /* VTR_VAL_BYTES */
struct pointer { uint64_t value; };                  /* VTR_VAL_POINTER */
struct timestamp { uint64_t value; };                /* VTR_VAL_TIME */

/* A LOG stream of a writer: the target of VTR_LOG. */
class LogStream {
public:
    LogStream() = default;
    /* Wraps an existing stream node (kind "LOG"). */
    LogStream(vtr_writer *w, uint32_t stream) : w_(w), stream_(stream) {}
    /* Declares a new LOG stream `name` under `parent` (VTR_NONE = top level). */
    LogStream(vtr_writer *w, uint32_t parent, const char *name) : w_(w), stream_(vtr_writer_add_log_stream(w, parent, name)) {}

    vtr_writer *writer() const { return w_; }
    uint32_t node() const { return stream_; }
    /* Transaction id attached as parent to the following messages (0 = none). */
    void set_parent(uint64_t tx) { parent_ = tx; }
    uint64_t parent() const { return parent_; }
    /* Messages below this severity are dropped before reaching the writer. */
    void set_min_severity(Severity s) { min_ = static_cast<uint8_t>(s); }
    bool enabled(Severity s) const { return static_cast<uint8_t>(s) >= min_; }
    /* Id of the last recorded message (a transaction id), 0 if none. */
    uint64_t last_id() const { return last_id_; }

    /* Internal: used by the macro. */
    uint64_t &last_id_slot() { return last_id_; }

private:
    vtr_writer *w_ = nullptr;
    uint32_t stream_ = VTR_NONE;
    uint64_t parent_ = 0;
    uint8_t min_ = 0;
    uint64_t last_id_ = 0;
};

namespace detail {

template <class T> using bare = std::remove_cv_t<std::remove_reference_t<T>>;

template <class T, class = void> struct arg_traits;

template <class T>
struct arg_traits<T, std::enable_if_t<std::is_same_v<T, bool>>> {
    static constexpr uint8_t tag = VTR_VAL_BOOL;
    static void fill(vtr_value &v, bool x) { v.b = x ? 1 : 0; }
};
template <class T>
struct arg_traits<T, std::enable_if_t<std::is_integral_v<T> && std::is_signed_v<T> && !std::is_same_v<T, bool>>> {
    static constexpr uint8_t tag = VTR_VAL_I64;
    static void fill(vtr_value &v, T x) { v.i = static_cast<int64_t>(x); }
};
template <class T>
struct arg_traits<T, std::enable_if_t<std::is_integral_v<T> && std::is_unsigned_v<T> && !std::is_same_v<T, bool>>> {
    static constexpr uint8_t tag = VTR_VAL_U64;
    static void fill(vtr_value &v, T x) { v.u = static_cast<uint64_t>(x); }
};
template <class T>
struct arg_traits<T, std::enable_if_t<std::is_enum_v<T>>> {
    static constexpr uint8_t tag = VTR_VAL_I64;
    static void fill(vtr_value &v, T x) { v.i = static_cast<int64_t>(static_cast<std::underlying_type_t<T>>(x)); }
};
template <class T>
struct arg_traits<T, std::enable_if_t<std::is_floating_point_v<T>>> {
    static constexpr uint8_t tag = VTR_VAL_F64;
    static void fill(vtr_value &v, T x) { v.f = static_cast<double>(x); }
};
template <>
struct arg_traits<const char *> {
    static constexpr uint8_t tag = VTR_VAL_TEXT;
    static void fill(vtr_value &v, const char *s) {
        v.data = reinterpret_cast<const uint8_t *>(s);
        v.len = s ? std::strlen(s) : 0;
    }
};
template <>
struct arg_traits<char *> : arg_traits<const char *> {};
template <size_t N>
struct arg_traits<char[N]> : arg_traits<const char *> {};
template <size_t N>
struct arg_traits<const char[N]> : arg_traits<const char *> {};
template <>
struct arg_traits<std::string> {
    static constexpr uint8_t tag = VTR_VAL_TEXT;
    static void fill(vtr_value &v, const std::string &s) {
        v.data = reinterpret_cast<const uint8_t *>(s.data());
        v.len = s.size();
    }
};
template <>
struct arg_traits<std::string_view> {
    static constexpr uint8_t tag = VTR_VAL_TEXT;
    static void fill(vtr_value &v, std::string_view s) {
        v.data = reinterpret_cast<const uint8_t *>(s.data());
        v.len = s.size();
    }
};
template <>
struct arg_traits<interned> {
    static constexpr uint8_t tag = VTR_VAL_STR;
    static void fill(vtr_value &v, interned s) { v.str_id = s.id; }
};
template <>
struct arg_traits<bytes> {
    static constexpr uint8_t tag = VTR_VAL_BYTES;
    static void fill(vtr_value &v, bytes b) {
        v.data = b.data;
        v.len = b.len;
    }
};
template <>
struct arg_traits<pointer> {
    static constexpr uint8_t tag = VTR_VAL_POINTER;
    static void fill(vtr_value &v, pointer p) { v.u = p.value; }
};
template <>
struct arg_traits<timestamp> {
    static constexpr uint8_t tag = VTR_VAL_TIME;
    static void fill(vtr_value &v, timestamp t) { v.u = t.value; }
};
template <class T>
struct arg_traits<T *, std::enable_if_t<!std::is_same_v<bare<T>, char>>> {
    static constexpr uint8_t tag = VTR_VAL_POINTER;
    static void fill(vtr_value &v, T *p) { v.u = static_cast<uint64_t>(reinterpret_cast<uintptr_t>(p)); }
};

template <class T> using traits_of = arg_traits<bare<T>>;

/* Row encoding of one argument (see vtr_writer_log_raw). */
inline uint8_t *put_varint(uint8_t *p, uint64_t v) {
    while (v >= 0x80) {
        *p++ = static_cast<uint8_t>(v | 0x80);
        v >>= 7;
    }
    *p++ = static_cast<uint8_t>(v);
    return p;
}
inline uint8_t *put_value(uint8_t *p, const vtr_value &v) {
    switch (v.tag) {
    case VTR_VAL_BOOL: *p++ = v.b ? 1 : 0; return p;
    case VTR_VAL_I64: return put_varint(p, (static_cast<uint64_t>(v.i) << 1) ^ static_cast<uint64_t>(v.i >> 63));
    case VTR_VAL_F64: std::memcpy(p, &v.f, 8); return p + 8;
    case VTR_VAL_STR: return put_varint(p, v.str_id);
    case VTR_VAL_TEXT:
    case VTR_VAL_BYTES:
        p = put_varint(p, v.len);
        if (v.len) std::memcpy(p, v.data, v.len);
        return p + v.len;
    default: return put_varint(p, v.u); /* U64, TIME, POINTER */
    }
}
inline size_t max_encoded(const vtr_value &v) {
    return (v.tag == VTR_VAL_TEXT || v.tag == VTR_VAL_BYTES) ? v.len + 10 : 10;
}

/* Number of `{...}` placeholders in a format string (`{{` escapes excluded). */
constexpr size_t count_placeholders(const char *s) {
    size_t n = 0;
    for (size_t i = 0; s[i] != 0; ++i) {
        if (s[i] == '{') {
            if (s[i + 1] == '{') {
                ++i;
            } else {
                ++n;
            }
        } else if (s[i] == '}' && s[i + 1] == '}') {
            ++i;
        }
    }
    return n;
}

/* The first argument is the format string; the remaining ones are counted. */
template <class Fmt, class... Args>
constexpr std::integral_constant<size_t, sizeof...(Args)> count_args(Fmt &&, Args &&...) { return {}; }

template <class... Args>
inline uint32_t register_site(LogStream &ls, Severity sev, const char *fmt, const char *file, uint32_t line, const char *func) {
    constexpr size_t n = sizeof...(Args);
    const uint8_t types[n == 0 ? 1 : n] = {traits_of<Args>::tag...};
    return vtr_writer_add_log_site(ls.writer(), ls.node(), static_cast<uint8_t>(sev), fmt, file, line, func, n, types, nullptr);
}

template <class... Args>
inline int emit(LogStream &ls, uint32_t site, uint64_t time, const Args &...args) {
    constexpr size_t n = sizeof...(Args);
    vtr_value vals[n == 0 ? 1 : n] = {};
    size_t i = 0;
    ((vals[i].tag = traits_of<Args>::tag, traits_of<Args>::fill(vals[i], args), ++i), ...);
    size_t bound = 0;
    for (size_t k = 0; k < n; ++k) bound += max_encoded(vals[k]);
    uint8_t stack[256];
    std::string heap;
    uint8_t *buf = stack;
    if (bound > sizeof stack) {
        heap.resize(bound);
        buf = reinterpret_cast<uint8_t *>(heap.data());
    }
    uint8_t *p = buf;
    for (size_t k = 0; k < n; ++k) p = put_value(p, vals[k]);
    return vtr_writer_log_raw(ls.writer(), site, time, ls.parent(), buf, static_cast<size_t>(p - buf), &ls.last_id_slot());
}

template <class Fmt, class... Args>
inline void log_call(uint32_t &site, LogStream &ls, Severity sev, uint64_t time, const char *file, uint32_t line, const char *func, const Fmt &fmt, const Args &...args) {
    if (!ls.enabled(sev)) return;
    if (site == VTR_NONE) site = register_site<Args...>(ls, sev, fmt, file, line, func);
    emit(ls, site, time, args...);
}

} // namespace detail

/* Reader side: renders a record with a small stack buffer, falling back to the heap. */
inline std::string format_log(const vtr_reader *r, const vtr_log_rec &rec) {
    char buf[256];
    size_t n = vtr_log_rec_format(r, &rec, buf, sizeof buf);
    if (n < sizeof buf) return std::string(buf, n);
    std::string s(n, '\0');
    vtr_log_rec_format(r, &rec, s.data(), n + 1);
    return s;
}

/* Visits log records; `f(const vtr_log_rec&)` returns false to stop. */
template <class F>
inline int for_each_log(const vtr_reader *r, uint32_t stream, uint8_t min_severity, uint64_t t0, uint64_t t1, F &&f) {
    struct Ctx { F *f; } ctx{&f};
    return vtr_reader_visit_log(
        r, stream, VTR_NONE, min_severity, t0, t1,
        [](void *user, const vtr_log_rec *rec) -> int { return (*static_cast<Ctx *>(user)->f)(*rec) ? 0 : 1; }, &ctx);
}

} // namespace vtr

#define VTR_LOG_FIRST_(a, ...) a
#define VTR_LOG_FIRST(...) VTR_LOG_FIRST_(__VA_ARGS__, 0)

/* VTR_LOG(stream, severity, time, "format", args...) */
#define VTR_LOG(stream, severity, time, ...)                                                                        \
    do {                                                                                                            \
        static_assert(::vtr::detail::count_placeholders(VTR_LOG_FIRST(__VA_ARGS__)) ==                             \
                          decltype(::vtr::detail::count_args(__VA_ARGS__))::value,                                  \
                      "VTR_LOG: number of {} placeholders must equal the number of arguments");                     \
        static uint32_t vtr_log_site_ = VTR_NONE;                                                                   \
        ::vtr::detail::log_call(vtr_log_site_, (stream), (severity), (time), __FILE__, __LINE__, __func__, __VA_ARGS__); \
    } while (0)

#define VTR_LOG_TRACE(stream, time, ...) VTR_LOG(stream, ::vtr::Severity::Trace, time, __VA_ARGS__)
#define VTR_LOG_DEBUG(stream, time, ...) VTR_LOG(stream, ::vtr::Severity::Debug, time, __VA_ARGS__)
#define VTR_LOG_INFO(stream, time, ...) VTR_LOG(stream, ::vtr::Severity::Info, time, __VA_ARGS__)
#define VTR_LOG_WARN(stream, time, ...) VTR_LOG(stream, ::vtr::Severity::Warn, time, __VA_ARGS__)
#define VTR_LOG_ERROR(stream, time, ...) VTR_LOG(stream, ::vtr::Severity::Error, time, __VA_ARGS__)
#define VTR_LOG_FATAL(stream, time, ...) VTR_LOG(stream, ::vtr::Severity::Fatal, time, __VA_ARGS__)

#endif /* VTR_LOG_HPP */
