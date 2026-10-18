#include "c_api_internal.hpp"
#include <bit>
#include <iomanip>
#include <sstream>
#include <unordered_set>

using namespace vtr;
using namespace vtr::detail;
using namespace vtr::capi;
namespace vtr::capi {
thread_local std::string error;
Value from_c(const vtr_value &v) {
  Value x;
  x.tag = static_cast<Tag>(v.tag);
  x.literal = v.str_id;
  x.scale = v.scale;
  switch (x.tag) {
  case Tag::null:
    break;
  case Tag::boolean:
    require(v.b <= 1, "invalid boolean", 1);
    x.number = v.b;
    break;
  case Tag::i64:
  case Tag::enumeration:
  case Tag::fixed:
    x.number = static_cast<std::uint64_t>(v.i);
    break;
  case Tag::u64:
  case Tag::time:
  case Tag::pointer:
  case Tag::ufixed:
    x.number = v.u;
    break;
  case Tag::f64:
    x.number = std::bit_cast<std::uint64_t>(v.f);
    break;
  case Tag::str:
    break;
  case Tag::bytes:
  case Tag::text:
    blob(x.payload, span(v.data, v.len));
    break;
  case Tag::bits:
  case Tag::logic:
  case Tag::logic9: {
    auto states = v.tag == 7 ? 2 : v.tag == 8 ? 4 : 9;
    SignalKind k{SignalKind::bits, v.width, static_cast<std::uint8_t>(states)};
    validate_kind(k);
    require(v.len == k.packed_size(), "packed value size mismatch", 1);
    put(x.payload, v.width);
    append(x.payload, span(v.data, v.len));
    break;
  }
  case Tag::list:
  case Tag::map: {
    Cursor r(span(v.data, v.len));
    x = decode_value(r, x.tag);
    r.done();
    break;
  }
  default:
    throw Error("invalid value tag", 1);
  }
  return x;
}
vtr_value to_c(const Value &v) {
  vtr_value out{};
  out.tag = static_cast<std::uint8_t>(v.tag);
  out.b = static_cast<std::uint8_t>(v.number);
  out.i = std::bit_cast<std::int64_t>(v.number);
  out.u = v.number;
  out.f = std::bit_cast<double>(v.number);
  out.str_id = v.literal;
  out.scale = v.scale;
  if (v.tag == Tag::bytes || v.tag == Tag::text) {
    Cursor r(v.payload);
    auto b = r.blob();
    out.data = b.data();
    out.len = b.size();
  } else if (v.tag >= Tag::bits && v.tag <= Tag::logic9) {
    Cursor r(v.payload);
    out.width = r.id();
    out.data = r.data.data();
    out.len = r.data.size();
  } else if (v.tag == Tag::list || v.tag == Tag::map) {
    out.data = v.payload.data();
    out.len = v.payload.size();
  }
  return out;
}
vtr_signal_value to_c(SignalValue v) {
  vtr_signal_value out{};
  out.kind = v.kind.type;
  out.width = v.kind.width;
  out.states = v.kind.states;
  out.data = v.data.data();
  out.len = v.data.size();
  if (v.kind.type == SignalKind::real)
    out.real = v.as_real();
  return out;
}
Attributes attrs(std::size_t n, const Id *keys, const vtr_value *values) {
  span(keys, n);
  span(values, n);
  Attributes out;
  out.reserve(n);
  for (std::size_t i = 0; i < n; ++i) {
    unique_attr(out, keys[i]);
    out.push_back({keys[i], from_c(values[i])});
  }
  return out;
}
Ending from_c(const vtr_ending &e) {
  require(e.kind <= 5, "invalid ending kind", 1);
  Ending out;
  out.kind = static_cast<Ending::Kind>(e.kind);
  out.signal = e.signal;
  out.code = e.code;
  out.status = e.status;
  out.address = e.address;
  out.thread = e.thread;
  out.dropped = e.dropped;
  out.sealed = e.sealed != 0;
  if (e.has_time)
    out.time = e.time;
  return out;
}
vtr_ending to_c(const Ending &e) {
  vtr_ending out{};
  out.kind = e.kind;
  out.signal = e.signal;
  out.code = e.code;
  out.status = e.status;
  out.address = e.address;
  out.thread = e.thread;
  out.dropped = e.dropped;
  out.sealed = e.sealed;
  out.has_time = e.time.has_value();
  out.time = e.time.value_or(0);
  return out;
}
std::size_t copy_text(std::string_view s, char *buf, std::size_t capacity) {
  if (capacity) {
    need(buf);
    auto n = std::min(capacity - 1, s.size());
    std::memcpy(buf, s.data(), n);
    buf[n] = 0;
  }
  return s.size();
}
} // namespace vtr::capi
extern "C" {
const char *vtr_last_error(void) { return error.c_str(); }
const char *vtr_version(void) { return "0.1.0-cpp"; }
void vtr_writer_options_default(vtr_writer_options *o) {
  if (!o)
    return;
  *o = {VTR_CODEC_LZ4, 3, 256, 1u << 24, 4u << 20, 1, 1, 0};
}
vtr_writer *vtr_writer_create(const char *path, const vtr_writer_options *o) {
  return result<vtr_writer *>(nullptr, [&] {
    WriterOptions opts;
    if (o) {
      opts.codec = static_cast<Codec>(o->codec);
      opts.compression_level = o->level;
      opts.group_size = o->group_size;
      opts.block_records = o->block_records;
      opts.tx_block_bytes = o->tx_block_bytes;
      opts.dedup = o->dedup != 0;
      opts.checksums = o->checksums != 0;
      opts.commit_interval_ms = o->commit_interval_ms;
    }
    return new vtr_writer(std::string(text(path)), opts);
  });
}
int vtr_writer_close(vtr_writer *w) {
  return vtr_writer_close_ending(w, nullptr);
}
int vtr_writer_close_ending(vtr_writer *w, const vtr_ending *end) {
  return status([&] {
    need(w);
    forget_writer(w);
    std::unique_ptr<vtr_writer> owner(w);
    auto e = end ? from_c(*end) : Ending{};
    w->writer.close(e);
  });
}
uint32_t vtr_writer_enter(vtr_writer *w) {
  return w ? w->entered.fetch_add(1) : 0;
}
void vtr_writer_leave(vtr_writer *w, uint32_t prev) {
  if (w)
    w->entered.store(prev);
}
int vtr_writer_set_file_attr(vtr_writer *w, const char *key,
                             const vtr_value *v) {
  return writer_status(w, [&] {
    auto &wr = need(w).writer;
    auto id = wr.intern(text(key));
    auto &list = wr.metadata().attrs;
    unique_attr(list, id);
    list.push_back({id, from_c(need(v))});
  });
}
uint32_t vtr_writer_intern(vtr_writer *w, const char *s) {
  return writer_result<Id>(w, none,
                           [&] { return need(w).writer.intern(text(s)); });
}
uint32_t vtr_writer_add_scope(vtr_writer *w, uint32_t parent, const char *name,
                              uint16_t type, const char *component) {
  return writer_result<Id>(w, none, [&] {
    return need(w).writer.add_scope(parent, text(name), type,
                                    optional_text(component));
  });
}
int vtr_writer_add_var(vtr_writer *w, uint32_t parent, const char *name,
                       uint16_t type, uint8_t direction, uint8_t kind,
                       uint32_t width, uint8_t states, uint32_t *node,
                       uint32_t *signal) {
  return writer_status(w, [&] {
    auto ids = need(w).writer.add_var(
        parent, text(name),
        {static_cast<SignalKind::Type>(kind), width, states}, type, direction);
    if (node)
      *node = ids.first;
    if (signal)
      *signal = ids.second;
  });
}
uint32_t vtr_writer_add_alias(vtr_writer *w, uint32_t parent, const char *name,
                              uint16_t type, uint8_t direction,
                              uint32_t signal) {
  return writer_result<Id>(w, none, [&] {
    return need(w).writer.add_alias(parent, text(name), signal, type,
                                    direction);
  });
}
uint32_t vtr_writer_add_generator(vtr_writer *w, uint32_t parent,
                                  const char *name, const char *kind) {
  return writer_result<Id>(w, none, [&] {
    return need(w).writer.add_generator(parent, text(name), text(kind));
  });
}
uint32_t vtr_writer_add_enum_table(vtr_writer *w, uint32_t parent,
                                   const char *name, size_t n,
                                   const char *const *literals,
                                   const char *const *values) {
  return writer_result<Id>(w, none, [&] {
    auto &wr = need(w).writer;
    span(literals, n);
    span(values, n);
    std::vector<std::pair<Id, Id>> entries;
    for (size_t i = 0; i < n; ++i)
      entries.emplace_back(wr.intern(text(literals[i])),
                           wr.intern(text(values[i])));
    return wr.add_enum_table(parent, text(name), entries);
  });
}
int vtr_writer_node_attr(vtr_writer *w, uint32_t node, const char *key,
                         const vtr_value *v) {
  return writer_status(w, [&] {
    auto &wr = need(w).writer;
    wr.node_attr(node, wr.intern(text(key)), from_c(need(v)));
  });
}
int vtr_writer_emit_words(vtr_writer *w, uint32_t sig, const uint32_t *words,
                          size_t n) {
  return writer_status(w,
                       [&] { need(w).writer.emit_words(sig, span(words, n)); });
}
int vtr_writer_emit_logic_str(vtr_writer *w, uint32_t sig, const char *s,
                              size_t len) {
  return writer_status(w, [&] {
    need(s);
    need(w).writer.emit_logic(sig, {s, len == SIZE_MAX ? std::strlen(s) : len});
  });
}
int vtr_writer_emit_packed(vtr_writer *w, uint32_t sig, uint8_t states,
                           const uint8_t *data, size_t len) {
  return status(
      [&] { need(w).writer.emit_packed(sig, states, span(data, len)); });
}
int vtr_writer_emit_varlen(vtr_writer *w, uint32_t sig, const uint8_t *data,
                           size_t len) {
  return writer_status(
      w, [&] { need(w).writer.emit_varlen(sig, span(data, len)); });
}
int vtr_writer_begin_tx(vtr_writer *w, uint32_t gen, uint64_t time,
                        uint64_t *out) {
  return writer_status(w, [&] {
    need(out);
    *out = need(w).writer.begin_tx(gen, time);
  });
}
int vtr_writer_tx_attr(vtr_writer *w, uint64_t tx, uint32_t key,
                       const vtr_value *v) {
  return writer_status(w, [&] {
    auto &value = need(v);
    if (value.tag == VTR_VAL_U64)
      need(w).writer.tx_attr(tx, key, value.u);
    else
      need(w).writer.tx_attr(tx, key, from_c(value));
  });
}
int vtr_writer_tx_event(vtr_writer *w, uint64_t tx, uint64_t time,
                        uint32_t name, size_t n, const uint32_t *keys,
                        const vtr_value *values) {
  return status(
      [&] { need(w).writer.tx_event(tx, time, name, attrs(n, keys, values)); });
}
int vtr_writer_tx_stage(vtr_writer *w, uint64_t tx, uint32_t name,
                        uint32_t lane, uint64_t begin, uint64_t end, size_t n,
                        const uint32_t *keys, const vtr_value *values) {
  return writer_status(w, [&] {
    need(w).writer.tx_stage(tx, name, lane, begin, end, attrs(n, keys, values));
  });
}
int vtr_writer_tx_stage_attr(vtr_writer *w, uint64_t tx, uint32_t key,
                             const vtr_value *v) {
  return status(
      [&] { need(w).writer.tx_stage_attr(tx, key, from_c(need(v))); });
}
int vtr_writer_relate(vtr_writer *w, uint32_t kind, uint64_t from, uint64_t to,
                      size_t n, const uint32_t *keys, const vtr_value *values) {
  return status(
      [&] { need(w).writer.relate(kind, from, to, attrs(n, keys, values)); });
}
uint32_t vtr_writer_add_log_site(vtr_writer *w, uint32_t scope,
                                 uint8_t severity, const char *fmt,
                                 const char *file, uint32_t line,
                                 const char *func, size_t n,
                                 const uint8_t *arg_types,
                                 const char *const *names) {
  return writer_result<Id>(w, none, [&] {
    span(arg_types, n);
    std::vector<Tag> types;
    std::vector<std::string_view> labels;
    for (size_t i = 0; i < n; ++i) {
      types.push_back(static_cast<Tag>(arg_types[i]));
      if (names)
        labels.push_back(text(names[i]));
    }
    return need(w).writer.add_log_site(scope, severity, text(fmt), types,
                                       labels, optional_text(file), line,
                                       optional_text(func));
  });
}
uint32_t vtr_writer_log_site_node(const vtr_writer *w, uint32_t site) {
  return result<Id>(none, [&] { return need(w).writer.log_site_node(site); });
}
int vtr_writer_log(vtr_writer *w, uint32_t site, uint64_t time, uint64_t parent,
                   size_t n, const vtr_value *args, uint64_t *out) {
  return writer_status(w, [&] {
    std::vector<Value> values;
    for (auto &v : span(args, n))
      values.push_back(from_c(v));
    auto id = need(w).writer.log(site, time, values, parent);
    if (out)
      *out = id;
  });
}
int vtr_writer_log_raw(vtr_writer *w, uint32_t site, uint64_t time,
                       uint64_t parent, const uint8_t *data, size_t len,
                       uint64_t *out) {
  return writer_status(w, [&] {
    auto id = need(w).writer.log_raw(site, time, span(data, len), parent);
    if (out)
      *out = id;
  });
}
uint32_t vtr_writer_add_clock(vtr_writer *w, uint32_t scope, const char *name) {
  return result<Id>(
      none, [&] { return need(w).writer.add_clock(scope, text(name)); });
}
uint32_t vtr_writer_clock_generator(const vtr_writer *w, uint32_t clock) {
  return result<Id>(none,
                    [&] { return need(w).writer.clock_generator(clock); });
}
vtr_reader *vtr_reader_open(const char *path) {
  return result<vtr_reader *>(
      nullptr, [&] { return new vtr_reader(std::string(text(path))); });
}
vtr_reader *vtr_reader_open_with_options(const char *path,
                                         const vtr_reader_options *options) {
  return result<vtr_reader *>(nullptr, [&] {
    return new vtr_reader(std::string(text(path)),
                          {options && options->verify_crc != 0});
  });
}
void vtr_reader_close(vtr_reader *r) { delete r; }
void vtr_reader_clear_cache(vtr_reader *r) {
  if (r)
    status([&] { r->reader.clear_cache(); });
}
int vtr_reader_meta(const vtr_reader *r, vtr_meta *out) {
  return status([&] {
    auto &rd = need(r).reader;
    auto &m = rd.metadata();
    auto &s = rd.stats();
    vtr_meta v{};
    v.timescale = m.timescale;
    v.time_zero = m.time_zero;
    v.file_type = m.file_type;
    v.group_size = m.group_size;
    v.has_time_range = s.time_range.has_value();
    if (s.time_range) {
      v.time_start = s.time_range->first;
      v.time_end = s.time_range->second;
    }
    v.version_major = 2;
    v.recovered = s.recovered;
    v.recovered_dropped = s.recovered_dropped;
    v.signal_count = rd.signals().size();
    v.node_count = rd.nodes().size();
    v.string_count = rd.strings().size();
    v.signal_block_count = s.signal_blocks;
    v.tx_block_count = s.tx_blocks;
    v.tx_count = s.transactions + s.logs;
    v.relation_count = s.relations;
    v.blackout_count = rd.blackouts().size();
    v.file_attr_count = m.attrs.size();
    v.log_block_count = s.log_blocks;
    v.log_count = s.logs;
    v.log_site_count = rd.log_sites().size();
    need(out) = v;
  });
}
int vtr_reader_ending(const vtr_reader *r, vtr_ending *out) {
  return status([&] { need(out) = to_c(need(r).reader.ending()); });
}
const char *vtr_reader_meta_string(const vtr_reader *r, int which,
                                   size_t *len) {
  return result<const char *>(nullptr, [&] {
    auto &m = need(r).reader.metadata();
    require(which >= 0 && which <= 2, "invalid metadata string", 1);
    auto &s = which == 0 ? m.writer : which == 1 ? m.date : m.comment;
    if (len)
      *len = s.size();
    return s.c_str();
  });
}
const char *vtr_reader_str(const vtr_reader *r, uint32_t id, size_t *len) {
  return result<const char *>(nullptr, [&] {
    auto s = need(r).reader.string(id);
    if (len)
      *len = s.size();
    return s.data();
  });
}
int vtr_reader_file_attr(const vtr_reader *r, uint32_t i, uint32_t *key,
                         vtr_value *value) {
  return status([&] {
    auto &a = at(need(r).reader.metadata().attrs, i);
    need(key) = a.key;
    need(value) = to_c(a.value);
  });
}
uint32_t vtr_reader_node_count(const vtr_reader *r) {
  return r ? static_cast<Id>(r->reader.nodes().size()) : 0;
}
uint32_t vtr_reader_signal_count(const vtr_reader *r) {
  return r ? static_cast<Id>(r->reader.signals().size()) : 0;
}
int vtr_reader_node(const vtr_reader *r, uint32_t id, vtr_node_info *out) {
  return status([&] {
    auto &rd = need(r).reader;
    auto &n = at(rd.nodes(), id);
    vtr_node_info v{};
    v.kind = n.kind;
    v.parent = n.parent;
    v.name = n.name;
    v.type_code = n.type;
    v.direction = n.direction;
    v.signal = n.signal;
    v.is_alias = n.alias;
    v.aux_str = n.aux;
    v.attr_count = n.attrs.size();
    v.entry_count = n.entries.size();
    for (auto &child : rd.nodes())
      v.child_count += child.parent == id;
    need(out) = v;
  });
}
int vtr_reader_node_attr(const vtr_reader *r, uint32_t id, uint32_t i,
                         uint32_t *key, vtr_value *value) {
  return status([&] {
    auto &a = at(at(need(r).reader.nodes(), id).attrs, i);
    need(key) = a.key;
    need(value) = to_c(a.value);
  });
}
int vtr_reader_enum_entry(const vtr_reader *r, uint32_t id, uint32_t i,
                          uint32_t *literal, uint32_t *value) {
  return status([&] {
    auto entry = at(at(need(r).reader.nodes(), id).entries, i);
    need(literal) = entry.first;
    need(value) = entry.second;
  });
}
size_t vtr_reader_children(const vtr_reader *r, uint32_t id, uint32_t *out,
                           size_t cap) {
  return result<size_t>(0, [&] {
    if (cap)
      need(out);
    auto nodes = need(r).reader.nodes();
    size_t count = 0;
    for (Id i = 0; i < nodes.size(); ++i)
      if (nodes[i].parent == id) {
        if (count < cap)
          need(out + count) = i;
        ++count;
      }
    return count;
  });
}
int vtr_reader_signal_kind(const vtr_reader *r, uint32_t sig, uint8_t *kind,
                           uint32_t *width, uint8_t *states) {
  return status([&] {
    auto k = at(need(r).reader.signals(), sig);
    need(kind) = k.type;
    need(width) = k.width;
    need(states) = k.states;
  });
}
int vtr_reader_signal_var_type(const vtr_reader *r, uint32_t sig,
                               uint16_t *type) {
  return status([&] {
    auto &rd = need(r).reader;
    need(type) = at(rd.nodes(), rd.signal_var(sig)).type;
  });
}
uint32_t vtr_reader_signal_var(const vtr_reader *r, uint32_t sig) {
  return result<Id>(none, [&] { return need(r).reader.signal_var(sig); });
}
int vtr_reader_find_node(const vtr_reader *r, const char *path, char sep,
                         uint32_t *out) {
  return status([&] {
    auto id = need(r).reader.find_node(text(path), sep);
    require(id != none, "node not found", 9);
    need(out) = id;
  });
}
int vtr_reader_find_signal(const vtr_reader *r, const char *path, char sep,
                           uint32_t *out) {
  return status([&] {
    auto id = need(r).reader.find_signal(text(path), sep);
    require(id != none, "signal not found", 9);
    need(out) = id;
  });
}
vtr_value_buf *vtr_value_buf_new(void) {
  return result<vtr_value_buf *>(nullptr, [] { return new vtr_value_buf; });
}
void vtr_value_buf_free(vtr_value_buf *b) { delete b; }
int vtr_value_buf_get(const vtr_value_buf *b, vtr_signal_value *out) {
  return status([&] {
    auto &v = need(b);
    need(out) = to_c(SignalValue{v.kind, v.data});
  });
}
const char *vtr_value_buf_ascii(vtr_value_buf *b) {
  return result<const char *>(nullptr, [&] {
    auto &v = need(b);
    v.ascii = SignalValue{v.kind, v.data}.ascii();
    return v.ascii.c_str();
  });
}
int vtr_reader_value_at(const vtr_reader *r, uint32_t sig, uint64_t time,
                        vtr_value_buf *b) {
  return status([&] {
    auto &rd = need(r).reader;
    auto &v = need(b);
    v.kind = at(rd.signals(), sig);
    v.data = rd.value_at(sig, time);
  });
}
int vtr_reader_changes(const vtr_reader *r, uint32_t sig, uint64_t t0,
                       uint64_t t1, vtr_change_cb cb, void *user) {
  return status([&] {
    require(cb, "null callback", 8);
    need(r).reader.changes(sig, t0, t1, [&](Time t, Id s, SignalValue v) {
      auto c = to_c(v);
      return cb(user, t, s, &c) == 0;
    });
  });
}
int vtr_reader_for_each_change(const vtr_reader *r, uint64_t t0, uint64_t t1,
                               vtr_change_cb cb, void *user) {
  return status([&] {
    require(cb, "null callback", 8);
    need(r).reader.changes(t0, t1, [&](Time t, Id s, SignalValue v) {
      auto c = to_c(v);
      return cb(user, t, s, &c) == 0;
    });
  });
}
vtr_signal_data *vtr_reader_load_signal(const vtr_reader *r, uint32_t sig) {
  return result<vtr_signal_data *>(nullptr, [&] {
    return new vtr_signal_data{need(r).reader.load_signal(sig)};
  });
}
int vtr_reader_load_signals(const vtr_reader *r, const uint32_t *sigs, size_t n,
                            vtr_signal_data **out) {
  return status([&] {
    auto &rd = need(r).reader;
    span(sigs, n);
    require(out || !n, "null output array", 8);
    for (auto sig : span(sigs, n))
      require(sig < rd.signals().size(), "unknown signal", 1);
    std::vector<std::unique_ptr<vtr_signal_data>> loaded;
    for (auto sig : span(sigs, n))
      loaded.push_back(std::make_unique<vtr_signal_data>(
          vtr_signal_data{rd.load_signal(sig)}));
    for (size_t i = 0; i < n; ++i)
      out[i] = loaded[i].release();
  });
}
vtr_signal_data *vtr_signal_data_clone(const vtr_signal_data *d) {
  return result<vtr_signal_data *>(
      nullptr, [&] { return new vtr_signal_data{need(d).data}; });
}
void vtr_signal_data_free(vtr_signal_data *d) { delete d; }
size_t vtr_signal_data_len(const vtr_signal_data *d) {
  return d ? d->data->times.size() : 0;
}
const uint64_t *vtr_signal_data_times(const vtr_signal_data *d) {
  return d ? d->data->times.data() : nullptr;
}
int vtr_signal_data_get(const vtr_signal_data *d, size_t i,
                        vtr_signal_value *out) {
  return status([&] { need(out) = to_c(need(d).data->at(i)); });
}
int vtr_signal_data_initial(const vtr_signal_data *d, vtr_signal_value *out) {
  return status([&] {
    auto &v = *need(d).data;
    need(out) = to_c(SignalValue{v.kind, v.initial});
  });
}
size_t vtr_signal_data_index_at(const vtr_signal_data *d, uint64_t time) {
  return result<size_t>(SIZE_MAX, [&] {
    auto &times = need(d).data->times;
    auto it = std::upper_bound(times.begin(), times.end(), time);
    return it == times.begin() ? SIZE_MAX
                               : static_cast<size_t>(it - times.begin() - 1);
  });
}
const uint64_t *vtr_reader_time_table(const vtr_reader *r, size_t *len) {
  return result<const uint64_t *>(nullptr, [&] {
    auto &tt = need(r).reader.time_table();
    need(len) = tt.size();
    return tt.data();
  });
}
int vtr_reader_blackout(const vtr_reader *r, uint32_t i, uint64_t *time,
                        int *active) {
  return status([&] {
    auto b = at(need(r).reader.blackouts(), i);
    need(time) = b.first;
    need(active) = b.second;
  });
}
int vtr_tx_get(const vtr_reader *r, const vtr_tx *tx, vtr_tx_info *out) {
  return status([&] {
    auto &rd = need(r).reader;
    auto &t = *need(tx).tx;
    vtr_tx_info v{};
    v.id = t.id;
    v.generator = t.generator;
    v.scope = at(rd.nodes(), t.generator).parent;
    v.begin = t.begin;
    v.end = t.end;
    v.status = t.status;
    v.kind = t.kind;
    v.has_parent = t.parent != 0;
    v.parent = t.parent;
    v.attr_count = t.attrs.size();
    v.event_count = t.events.size();
    v.stage_count = t.stages.size();
    need(out) = v;
  });
}
int vtr_tx_attr(const vtr_tx *tx, uint32_t i, uint32_t *key, vtr_value *value) {
  return status([&] {
    auto &a = at(need(tx).tx->attrs, i);
    need(key) = a.key;
    need(value) = to_c(a.value);
  });
}
int vtr_tx_event(const vtr_tx *tx, uint32_t i, uint64_t *time, uint32_t *name,
                 uint32_t *count) {
  return status([&] {
    auto &e = at(need(tx).tx->events, i);
    need(time) = e.time;
    need(name) = e.name;
    need(count) = e.attrs.size();
  });
}
int vtr_tx_event_attr(const vtr_tx *tx, uint32_t i, uint32_t j, uint32_t *key,
                      vtr_value *value) {
  return status([&] {
    auto &a = at(at(need(tx).tx->events, i).attrs, j);
    need(key) = a.key;
    need(value) = to_c(a.value);
  });
}
int vtr_tx_stage(const vtr_tx *tx, uint32_t i, uint32_t *name, uint32_t *lane,
                 uint64_t *begin, uint64_t *end, int *has_end,
                 uint32_t *count) {
  return status([&] {
    auto &s = at(need(tx).tx->stages, i);
    need(name) = s.name;
    need(lane) = s.lane;
    need(begin) = s.begin;
    need(end) = s.end.value_or(need(tx).tx->end);
    need(has_end) = s.end.has_value();
    need(count) = s.attrs.size();
  });
}
int vtr_tx_stage_attr(const vtr_tx *tx, uint32_t i, uint32_t j, uint32_t *key,
                      vtr_value *value) {
  return status([&] {
    auto &a = at(at(need(tx).tx->stages, i).attrs, j);
    need(key) = a.key;
    need(value) = to_c(a.value);
  });
}
int vtr_reader_visit_transactions(const vtr_reader *r, uint32_t generator,
                                  uint32_t scope, uint64_t t0, uint64_t t1,
                                  vtr_tx_cb cb, void *user) {
  return status([&] {
    require(cb, "null callback", 8);
    need(r).reader.transactions(query(generator, scope, t0, t1),
                                [&](const Transaction &t) {
                                  vtr_tx tx{&t};
                                  return cb(user, &tx) == 0;
                                });
  });
}
int vtr_reader_transaction(const vtr_reader *r, uint64_t id, vtr_tx_cb cb,
                           void *user) {
  return status([&] {
    require(cb, "null callback", 8);
    auto found = need(r).reader.transaction(id, [&](const Transaction &t) {
      vtr_tx tx{&t};
      cb(user, &tx);
    });
    require(found, "transaction not found", 9);
  });
}
int vtr_reader_transaction_generator(const vtr_reader *r, uint64_t id,
                                     uint32_t *out) {
  return status([&] {
    need(out);
    auto found = need(r).reader.transaction(
        id, [&](const Transaction &t) { *out = t.generator; });
    require(found, "transaction not found", 9);
  });
}
int vtr_reader_transaction_generators(const vtr_reader *r, const uint64_t *ids,
                                      size_t n, uint32_t *out) {
  return status([&] {
    auto &rd = need(r).reader;
    span(ids, n);
    require(out || !n, "null output array", 8);
    for (size_t i = 0; i < n; ++i) {
      out[i] = none;
      rd.transaction(ids[i],
                     [&](const Transaction &t) { out[i] = t.generator; });
    }
  });
}
int vtr_reader_relations(const vtr_reader *r, uint64_t id, int direction,
                         vtr_relation_cb cb, void *user) {
  return status([&] {
    require(cb, "null callback", 8);
    require(direction == 0 || direction == 1, "invalid relation direction", 1);
    need(r).reader.relations(id, direction != 0, [&](auto &rel) {
      std::vector<Id> keys;
      std::vector<vtr_value> values;
      for (auto &a : rel.attrs) {
        keys.push_back(a.key);
        values.push_back(to_c(a.value));
      }
      return cb(user, rel.kind, rel.from, rel.to, keys.size(), keys.data(),
                values.data()) == 0;
    });
  });
}
uint32_t vtr_reader_log_site_count(const vtr_reader *r) {
  return r ? static_cast<Id>(r->reader.log_sites().size()) : 0;
}
uint64_t vtr_reader_log_count(const vtr_reader *r) {
  return r ? r->reader.stats().logs : 0;
}
int vtr_reader_log_site(const vtr_reader *r, uint32_t i,
                        vtr_log_site_info *out) {
  return status([&] {
    auto &s = at(need(r).reader.log_sites(), i);
    need(out) = {
        s.node, s.scope,    s.severity, s.format,
        s.file, s.function, s.line,     static_cast<Id>(s.types.size())};
  });
}
int vtr_reader_log_site_arg(const vtr_reader *r, uint32_t i, uint32_t j,
                            uint8_t *type, uint32_t *name) {
  return status([&] {
    auto &s = at(need(r).reader.log_sites(), i);
    need(type) = static_cast<std::uint8_t>(at(s.types, j));
    need(name) = at(s.names, j);
  });
}
uint32_t vtr_reader_log_site_of(const vtr_reader *r, uint32_t generator) {
  return result<Id>(none, [&] {
    auto sites = need(r).reader.log_sites();
    for (Id i = 0; i < sites.size(); ++i)
      if (sites[i].node == generator)
        return i;
    return none;
  });
}
int vtr_reader_visit_log(const vtr_reader *r, uint32_t scope,
                         uint32_t generator, uint8_t severity, uint64_t t0,
                         uint64_t t1, vtr_log_cb cb, void *user) {
  return status([&] {
    require(cb, "null callback", 8);
    auto &rd = need(r).reader;
    rd.logs(query(generator, scope, t0, t1), severity,
            [&](const LogRecord &log) {
              auto &s = rd.log_sites()[log.site];
              vtr_log_rec rec{log.id,     log.time,
                              log.parent, log.site,
                              s.node,     s.scope,
                              s.severity, static_cast<Id>(log.args.size()),
                              &log};
              return cb(user, &rec) == 0;
            });
  });
}
int vtr_log_rec_arg(const vtr_log_rec *rec, uint32_t i, vtr_value *out) {
  return status([&] {
    auto &log = need(static_cast<const LogRecord *>(need(rec).inner_));
    need(out) = to_c(at(log.args, i));
  });
}
size_t vtr_log_rec_format(const vtr_reader *r, const vtr_log_rec *rec,
                          char *buf, size_t cap) {
  return result<size_t>(0, [&] {
    auto &log = need(static_cast<const LogRecord *>(need(rec).inner_));
    return copy_text(need(r).reader.format_log(log), buf, cap);
  });
}
int vtr_recover(const char *in, const char *out, int64_t *dropped) {
  return status([&] {
    auto input = std::string(text(in)), output = std::string(text(out));
    Reader reader(input);
    if (dropped)
      *dropped = static_cast<std::int64_t>(reader.stats().recovered_dropped);
    recover(input, output);
  });
}
size_t vtr_ending_format(const vtr_ending *e, char *buf, size_t cap) {
  return result<size_t>(0, [&] {
    auto ending = from_c(need(e));
    std::ostringstream s;
    auto signal = [&] {
      switch (ending.signal) {
      case 2:
        return std::string("SIGINT");
      case 6:
        return std::string("SIGABRT");
      case 11:
        return std::string("SIGSEGV");
      case 15:
        return std::string("SIGTERM");
      default:
        return "signal " + std::to_string(ending.signal);
      }
    };
    switch (ending.kind) {
    case Ending::closed:
      s << "closed";
      break;
    case Ending::stopped:
      s << "stopped by " << signal();
      break;
    case Ending::exited:
      s << "exited with status " << ending.status;
      break;
    case Ending::crashed:
      s << "crashed by " << signal() << " (address 0x" << std::hex
        << ending.address << std::dec << ", thread " << ending.thread << ")";
      break;
    case Ending::recovered:
      s << "recovered by scanning, " << ending.dropped << " bytes dropped";
      break;
    case Ending::poisoned:
      s << "sealed after writer failure";
      break;
    }
    return copy_text(s.str(), buf, cap);
  });
}
}

extern "C" {
int vtr_writer_set_timescale(vtr_writer *w, int8_t value) {
  return writer_status(w, [&] { need(w).writer.metadata().timescale = value; });
}
int vtr_writer_set_time_zero(vtr_writer *w, int64_t value) {
  return writer_status(w, [&] { need(w).writer.metadata().time_zero = value; });
}
int vtr_writer_set_file_type(vtr_writer *w, uint8_t value) {
  return writer_status(w, [&] { need(w).writer.metadata().file_type = value; });
}
int vtr_writer_set_writer_name(vtr_writer *w, const char *value) {
  return writer_status(w,
                       [&] { need(w).writer.metadata().writer = text(value); });
}
int vtr_writer_set_date(vtr_writer *w, const char *value) {
  return writer_status(w,
                       [&] { need(w).writer.metadata().date = text(value); });
}
int vtr_writer_set_comment(vtr_writer *w, const char *value) {
  return writer_status(
      w, [&] { need(w).writer.metadata().comment = text(value); });
}
int vtr_writer_set_time(vtr_writer *w, uint64_t t) {
  return writer_status(w, [&] { need(w).writer.set_time(t); });
}
int vtr_writer_dump_off(vtr_writer *w) {
  return writer_status(w, [&] { need(w).writer.dump(false); });
}
int vtr_writer_dump_on(vtr_writer *w) {
  return writer_status(w, [&] { need(w).writer.dump(true); });
}
int vtr_writer_emit_bit(vtr_writer *w, uint32_t sig, uint8_t code) {
  return writer_status(w, [&] { need(w).writer.emit_bit(sig, code); });
}
int vtr_writer_emit_u64(vtr_writer *w, uint32_t sig, uint64_t value) {
  return writer_status(w, [&] { need(w).writer.emit(sig, value); });
}
int vtr_writer_emit_real(vtr_writer *w, uint32_t sig, double value) {
  return writer_status(w, [&] { need(w).writer.emit_real(sig, value); });
}
int vtr_writer_flush(vtr_writer *w) {
  return writer_status(w, [&] { need(w).writer.flush(); });
}
int vtr_writer_set_tx_parent(vtr_writer *w, uint64_t tx, uint64_t parent) {
  return writer_status(w, [&] { need(w).writer.tx_parent(tx, parent); });
}
int vtr_writer_set_tx_kind(vtr_writer *w, uint64_t tx, uint8_t kind) {
  return writer_status(w, [&] { need(w).writer.tx_kind(tx, kind); });
}
int vtr_writer_tx_stage_begin(vtr_writer *w, uint64_t tx, uint32_t name,
                              uint32_t lane, uint64_t time) {
  return writer_status(
      w, [&] { need(w).writer.tx_stage(tx, name, lane, time, {}); });
}
int vtr_writer_tx_stage_end(vtr_writer *w, uint64_t tx, uint32_t name,
                            uint32_t lane, uint64_t time) {
  return writer_status(
      w, [&] { need(w).writer.tx_stage_end(tx, name, lane, time); });
}
int vtr_writer_end_tx(vtr_writer *w, uint64_t tx, uint64_t time, uint8_t st) {
  return writer_status(w, [&] { need(w).writer.end_tx(tx, time, st); });
}
int vtr_writer_clock_run(vtr_writer *w, uint32_t clock, uint64_t first,
                         uint64_t period) {
  return writer_status(w,
                       [&] { need(w).writer.clock_run(clock, first, period); });
}
int vtr_writer_clock_stop(vtr_writer *w, uint32_t clock, uint64_t time) {
  return writer_status(w, [&] { need(w).writer.clock_stop(clock, time); });
}
}
