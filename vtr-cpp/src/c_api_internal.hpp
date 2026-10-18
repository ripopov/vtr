#pragma once
#include "format.hpp"
#include "vtr.h"
#include <atomic>
#include <mutex>
#include <optional>

struct vtr_writer {
  vtr::Writer writer;
  std::string path;
  std::atomic<unsigned> entered{0};
  std::atomic<bool> sealed{false};
  explicit vtr_writer(const std::string &p, vtr::WriterOptions options)
      : writer(p, options), path(p) {}
};
struct vtr_reader {
  vtr::Reader reader;
  explicit vtr_reader(const std::string &p, vtr::ReaderOptions options = {})
      : reader(p, options) {}
};
struct vtr_tx {
  const vtr::Transaction *tx;
};
struct vtr_value_buf {
  vtr::SignalKind kind;
  vtr::Bytes data;
  std::string ascii;
};
struct vtr_signal_data {
  std::shared_ptr<const vtr::SignalData> data;
};
namespace vtr::capi {
extern thread_local std::string error;
template <class F> int status(F &&f) noexcept {
  try {
    f();
    return VTR_OK;
  } catch (const Error &e) {
    error = e.what();
    return e.code;
  } catch (const std::exception &e) {
    error = e.what();
    return VTR_ERR_STATE;
  } catch (...) {
    error = "unknown native exception";
    return VTR_ERR_STATE;
  }
}
template <class T, class F> T result(T fallback, F &&f) noexcept {
  T out = fallback;
  status([&] { out = f(); });
  return out;
}
template <class T> T &need(T *p) {
  detail::require(p, "null argument", VTR_ERR_NULL);
  return *p;
}
inline std::string_view text(const char *p) {
  need(p);
  return p;
}
inline std::string_view optional_text(const char *p) {
  return p ? std::string_view(p) : std::string_view{};
}
template <class T> std::span<const T> span(const T *p, std::size_t n) {
  detail::require(p || !n, "null array", VTR_ERR_NULL);
  return {p, n};
}
template <class T> const T &at(std::span<const T> v, std::size_t i) {
  detail::require(i < v.size(), "index out of range", VTR_ERR_NOT_FOUND);
  return v[i];
}
template <class T> const T &at(const std::vector<T> &v, std::size_t i) {
  return at(std::span<const T>(v), i);
}
template <class F> int writer_status(vtr_writer *w, F &&f) noexcept {
  return status([&] {
    need(w);
    struct Call {
      vtr_writer *writer;
      ~Call() { writer->entered.fetch_sub(1); }
    } call{w};
    w->entered.fetch_add(1);
    detail::require(!w->sealed.load(), "writer was sealed by the guard",
                    VTR_ERR_STATE);
    f();
  });
}
template <class T, class F>
T writer_result(vtr_writer *w, T fallback, F &&f) noexcept {
  T value = fallback;
  writer_status(w, [&] { value = f(); });
  return value;
}
Value from_c(const vtr_value &v);
vtr_value to_c(const Value &v);
vtr_signal_value to_c(SignalValue v);
Attributes attrs(std::size_t n, const Id *keys, const vtr_value *values);
Ending from_c(const vtr_ending &e);
vtr_ending to_c(const Ending &e);
std::size_t copy_text(std::string_view s, char *buf, std::size_t capacity);
inline Query query(Id generator, Id scope, Time begin, Time end) {
  return {generator, scope, begin, end == 0 ? forever : end};
}
void forget_writer(vtr_writer *writer);
} // namespace vtr::capi
