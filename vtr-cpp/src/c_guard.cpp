#include "containers.hpp"
#include "c_api_internal.hpp"
#include <chrono>
#include <csignal>
#include <cstdlib>
#include <fcntl.h>
#include <filesystem>
#include <fstream>
#include <linux/futex.h>
#include <sys/syscall.h>
#include <thread>
#include <unistd.h>
using namespace vtr;
using namespace vtr::detail;
using namespace vtr::capi;
namespace {
struct Notice {
  int signal, code;
  std::uint64_t address, thread;
};
struct Guard {
  std::mutex mutex;
  std::vector<vtr_writer *> writers;
  HashMap<std::uint64_t, std::pair<void (*)(void *, int), void *>>
      callbacks;
  std::uint64_t next = 1;
  int pipe[2]{-1, -1};
  std::atomic<int> stop{0};
  bool installed = false;
  std::atomic<int> terminal_signal{0};
  vtr_guard_options options{};
  Ending ending;
};
Guard &guard() {
  static auto *value = new Guard;
  return *value;
}
std::atomic<int> parked{0};
static_assert(std::atomic<int>::is_always_lock_free);
void signal_handler(int signal, siginfo_t *info, void *) {
  int saved = errno;
  Notice n{signal, info ? info->si_code : 0,
           info ? reinterpret_cast<std::uint64_t>(info->si_addr) : 0,
           static_cast<std::uint64_t>(syscall(SYS_gettid))};
  auto bytes = ::write(guard().pipe[1], &n, sizeof(n));
  if (bytes != sizeof(n))
    _Exit(128 + signal);
  if (signal != SIGINT && signal != SIGTERM && signal != SIGHUP) {
    for (;;)
      syscall(SYS_futex, reinterpret_cast<int *>(&parked), FUTEX_WAIT_PRIVATE,
              0, nullptr, nullptr, 0);
  }
  errno = saved;
}
void write_all(int fd, View data) {
  while (!data.empty()) {
    auto n = ::write(fd, data.data(), data.size());
    if (n < 0 && errno == EINTR)
      continue;
    require(n > 0, "cannot seal VTR file", 3);
    data = data.subspan(n);
  }
}
void seal_file_impl(const std::string &path, Ending ending) {
  Reader r(path);
  std::ifstream source(path, std::ios::binary);
  std::vector<Section> dir;
  std::uint64_t offset = 32;
  auto total = std::filesystem::file_size(path);
  while (offset + 24 <= total) {
    std::array<std::uint8_t, 24> h{};
    source.seekg(offset);
    source.read(reinterpret_cast<char *>(h.data()), 24);
    Cursor c(h);
    Section s;
    s.kind = c.fixed<Id>();
    s.flags = c.fixed<Id>();
    s.offset = offset;
    s.length = c.fixed<std::uint64_t>();
    auto crc = c.fixed<Id>();
    if (!s.kind || s.kind == 7 || s.length > total - offset - 24)
      break;
    Bytes raw(s.length);
    source.read(reinterpret_cast<char *>(raw.data()), raw.size());
    if (crc && checksum(raw) != crc)
      break;
    if (s.kind == 2 || s.kind == 3) {
      auto data = decompress(raw);
      Cursor p(data);
      s.aux0 = p.var();
      s.aux1 = p.var();
    } else if (s.kind == 4) {
      s.aux0 = load<Time>(raw);
      s.aux1 = load<Time>(View(raw).subspan(8));
    } else if (s.kind == 5) {
      s.aux0 = load<Time>(View(raw).subspan(32));
      s.aux1 = load<Time>(View(raw).subspan(40));
    } else if (s.kind == 8) {
      s.aux0 = load<Time>(View(raw).subspan(24));
      s.aux1 = load<Time>(View(raw).subspan(32));
    }
    dir.push_back(s);
    offset += 24 + s.length;
  }
  int fd = ::open(path.c_str(), O_WRONLY);
  require(fd >= 0, "cannot seal trace", 3);
  try {
    require(ftruncate(fd, offset) == 0 && lseek(fd, offset, SEEK_SET) >= 0,
            "cannot truncate incomplete trace", 3);
    auto section = [&](Id kind, View payload, Time first = 0, Time last = 0,
                       Id flags = 0) {
      Bytes h;
      fixed(h, kind);
      fixed(h, flags);
      fixed<std::uint64_t>(h, payload.size());
      fixed(h, checksum(payload));
      fixed<Id>(h, 0);
      write_all(fd, h);
      write_all(fd, payload);
      dir.push_back({kind, flags, offset, payload.size(), first, last});
      offset += 24 + payload.size();
    };
    std::vector<std::string> strings;
    auto intern = [&](std::string_view s) {
      auto id = static_cast<Id>(r.strings().size() + strings.size());
      strings.emplace_back(s);
      return id;
    };
    auto scope_name = intern("vtr.run"), kind = intern("LOG"),
         args_key = intern("log.args"), names_key = intern("log.names"),
         severity_key = intern("log.severity");
    std::string_view format;
    std::vector<Value> args;
    std::vector<std::string_view> names;
    if (ending.kind == Ending::crashed) {
      format = "run crashed by signal {} (code {}, address {}, thread {}, "
               "sealed {})";
      names = {"signal", "code", "address", "thread", "sealed"};
      auto address = Value(ending.address);
      address.tag = Tag::pointer;
      args = {Value(std::int64_t(ending.signal)),
              Value(std::int64_t(ending.code)), address, Value(ending.thread),
              Value(true)};
    } else if (ending.kind == Ending::stopped) {
      format = "run stopped by signal {}";
      names = {"signal"};
      args = {Value(std::int64_t(ending.signal))};
    } else if (ending.kind == Ending::recovered) {
      format = "run recovered by scanning, {} bytes dropped";
      names = {"dropped"};
      args = {Value(ending.dropped)};
    } else {
      format = "run exited with status {}";
      names = {"status"};
      args = {Value(std::int64_t(ending.status))};
    }
    auto fmt = intern(format);
    std::vector<Value> tags, labels;
    for (size_t i = 0; i < args.size(); ++i) {
      tags.emplace_back(std::uint64_t(args[i].tag));
      labels.push_back(Value::string(intern(names[i])));
    }
    Bytes raw;
    put(raw, r.strings().size());
    put(raw, strings.size());
    for (auto &s : strings)
      blob(raw, s);
    section(2, compress(raw, Codec::lz4, 0), r.strings().size(),
            strings.size());
    auto scope = static_cast<Id>(r.nodes().size()), gen = scope + 1;
    raw.clear();
    put(raw, scope);
    put(raw, 2);
    raw.push_back(1);
    put(raw, 0);
    put(raw, scope_name);
    put(raw, 64);
    put(raw, 0);
    put(raw, 0);
    raw.push_back(3);
    put(raw, 1);
    put(raw, fmt);
    put(raw, kind);
    Attributes attrs{{args_key, Value::list(tags)},
                     {names_key, Value::list(labels)},
                     {severity_key, Value(std::uint64_t(5))}};
    encode_attrs(raw, attrs);
    section(3, compress(raw, Codec::lz4, 0), scope, 2);
    std::uint64_t id = 0;
    for (auto &s : dir)
      if (s.kind == 5 || s.kind == 8) {
        source.clear();
        source.seekg(s.offset + 24 + (s.kind == 5 ? 24 : 16));
        std::array<std::uint8_t, 8> b{};
        source.read(reinterpret_cast<char *>(b.data()), 8);
        id = std::max(id, load<std::uint64_t>(b));
      }
    ++id;
    Time time = ending.time.value_or(
        r.stats().time_range ? r.stats().time_range->second : 0);
    std::array<Bytes, 9> columns;
    put(columns[0], 0);
    sint(columns[1], time);
    sint(columns[2], id);
    put(columns[3], 0);
    for (auto &a : args)
      encode_payload(columns[4], a);
    put(columns[7], 0);
    auto packed = compress(pack_columns(columns), Codec::lz4, 0);
    raw.clear();
    for (auto n : {std::uint64_t(1), id, id, time, time})
      fixed(raw, n);
    fixed<Id>(raw, 1);
    fixed<Id>(raw, packed.size());
    fixed(raw, gen);
    append(raw, packed);
    section(8, raw, time, time, 1);
    raw.clear();
    fixed<std::uint64_t>(raw, dir.size());
    for (auto &s : dir) {
      fixed(raw, s.kind);
      fixed(raw, s.flags);
      fixed(raw, s.offset);
      fixed(raw, s.length);
      fixed(raw, s.aux0);
      fixed(raw, s.aux1);
    }
    auto off = offset;
    section(7, raw);
    raw.clear();
    fixed(raw, off);
    fixed(raw, offset + 24);
    append(raw, trailer);
    write_all(fd, raw);
    ::close(fd);
  } catch (...) {
    ::close(fd);
    throw;
  }
}
void finish_writers(Ending ending) {
  auto &g = guard();
  std::lock_guard lock(g.mutex);
  g.ending = ending;
  for (auto *w : g.writers) {
    w->sealed.store(true);
    try {
      if (w->entered.load()) {
        ending.sealed = true;
        auto snapshot = w->path + ".seal." + std::to_string(getpid());
        std::filesystem::copy_file(
            w->path, snapshot,
            std::filesystem::copy_options::overwrite_existing);
        try {
          seal_file_impl(snapshot, ending);
          std::filesystem::rename(snapshot, w->path);
        } catch (...) {
          std::error_code error;
          std::filesystem::remove(snapshot, error);
          throw;
        }
      } else
        w->writer.close(ending);
    } catch (...) {
    }
  }
  g.writers.clear();
}
void process_exit(int status, void *) {
  Ending ending;
  ending.kind = Ending::exited;
  ending.status = status;
  finish_writers(ending);
}
[[noreturn]] void terminate_signal(int signal) {
  std::signal(signal, SIG_DFL);
  sigset_t mask;
  sigemptyset(&mask);
  sigaddset(&mask, signal);
  pthread_sigmask(SIG_UNBLOCK, &mask, nullptr);
  ::raise(signal);
  _Exit(128 + signal);
}
void watchdog() {
  auto &g = guard();
  std::optional<std::chrono::steady_clock::time_point> stop, terminal;
  for (;;) {
    std::this_thread::sleep_for(std::chrono::milliseconds(10));
    auto now = std::chrono::steady_clock::now();
    if (g.stop.load() && !stop)
      stop = now;
    if (stop && g.options.stop_grace_ms &&
        now - *stop >= std::chrono::milliseconds(g.options.stop_grace_ms)) {
      Notice notice{g.stop.load(), INT_MIN, 0, 0};
      if (::write(g.pipe[1], &notice, sizeof(notice)) != sizeof(notice))
        terminate_signal(notice.signal);
      stop.reset();
    }
    if (g.terminal_signal.load() && !terminal)
      terminal = now;
    if (terminal &&
        now - *terminal >= std::chrono::milliseconds(g.options.deadline_ms))
      terminate_signal(g.terminal_signal.load());
  }
}
void coordinator() {
  auto &g = guard();
  for (;;) {
    Notice notice{};
    auto n = ::read(g.pipe[0], &notice, sizeof(notice));
    if (n < 0 && errno == EINTR)
      continue;
    if (n != sizeof(notice))
      return;
    bool stop = notice.signal == SIGINT || notice.signal == SIGTERM ||
                notice.signal == SIGHUP;
    if (stop && notice.code != INT_MIN) {
      g.stop = notice.signal;
      std::vector<std::pair<void (*)(void *, int), void *>> callbacks;
      {
        std::lock_guard lock(g.mutex);
        g.ending.kind = Ending::stopped;
        g.ending.signal = notice.signal;
        for (auto &[id, cb] : g.callbacks) {
          (void)id;
          callbacks.push_back(cb);
        }
      }
      for (auto cb : callbacks)
        cb.first(cb.second, notice.signal);
      continue;
    }
    Ending e;
    g.terminal_signal = notice.signal;
    e.kind = stop ? Ending::stopped : Ending::crashed;
    e.signal = notice.signal;
    e.code = notice.code;
    e.address = notice.address;
    e.thread = notice.thread;
    finish_writers(e);
    terminate_signal(notice.signal);
  }
}
} // namespace
namespace vtr::detail {
void seal_file(const std::string &path, Ending ending) {
  seal_file_impl(path, ending);
}
} // namespace vtr::detail
namespace vtr::capi {
void forget_writer(vtr_writer *w) {
  auto &g = guard();
  std::lock_guard lock(g.mutex);
  std::erase(g.writers, w);
}
} // namespace vtr::capi
extern "C" {
void vtr_guard_options_default(vtr_guard_options *o) {
  if (o)
    *o = {2000, 1, 1, 2000, 1};
}
int vtr_guard_install(const vtr_guard_options *options, int *active) {
  return status([&] {
    auto &g = guard();
    std::lock_guard lock(g.mutex);
    if (!g.installed) {
      if (options)
        g.options = *options;
      else
        vtr_guard_options_default(&g.options);
      require(::pipe(g.pipe) == 0, "cannot create signal notification pipe", 3);
      std::thread(coordinator).detach();
      std::thread(watchdog).detach();
      struct sigaction action{};
      action.sa_sigaction = signal_handler;
      sigemptyset(&action.sa_mask);
      action.sa_flags = SA_SIGINFO | SA_RESTART | SA_ONSTACK;
      auto install = [&](int signal) {
        require(sigaction(signal, &action, nullptr) == 0,
                "cannot install signal handler", 3);
      };
      vtr_guard_thread_init();
      if (g.options.crashes)
        for (auto signal : {SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGABRT})
          install(signal);
      if (g.options.stops)
        for (auto signal : {SIGINT, SIGTERM, SIGHUP})
          install(signal);
      if (g.options.exit)
        ::on_exit(process_exit, nullptr);
      g.installed = true;
    }
    if (active)
      *active = 1;
  });
}
int vtr_guard_watch(vtr_writer *w) {
  return status([&] {
    need(w);
    auto &g = guard();
    std::lock_guard lock(g.mutex);
    require(g.installed, "guard is not installed", 2);
    if (std::find(g.writers.begin(), g.writers.end(), w) == g.writers.end())
      g.writers.push_back(w);
  });
}
void vtr_guard_unwatch(vtr_writer *w) { forget_writer(w); }
void vtr_guard_thread_init(void) {
  status([] {
    thread_local Bytes alternate_stack;
    if (!alternate_stack.empty())
      return;
    alternate_stack.resize(std::max<std::size_t>(SIGSTKSZ, 64 * 1024));
    stack_t stack{};
    stack.ss_sp = alternate_stack.data();
    stack.ss_size = alternate_stack.size();
    require(sigaltstack(&stack, nullptr) == 0,
            "cannot install alternate signal stack", 3);
  });
}
int vtr_guard_stop_requested(void) { return guard().stop.load(); }
uint64_t vtr_guard_add_stop_callback(void (*cb)(void *, int), void *user) {
  return result<std::uint64_t>(0, [&] {
    require(cb, "null callback", 8);
    auto &g = guard();
    std::lock_guard lock(g.mutex);
    auto id = g.next++;
    g.callbacks.emplace(id, std::make_pair(cb, user));
    return id;
  });
}
void vtr_guard_remove_stop_callback(uint64_t id) {
  auto &g = guard();
  std::lock_guard lock(g.mutex);
  g.callbacks.erase(id);
}
void vtr_guard_ending(vtr_ending *out) {
  if (out) {
    auto &g = guard();
    std::lock_guard lock(g.mutex);
    *out = to_c(g.ending);
  }
}
}
