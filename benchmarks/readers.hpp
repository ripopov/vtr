#pragma once

#include "writers.hpp"
#include "ftr_reader.hpp"
#include <exception>
#include <functional>
#include <sstream>

namespace bench {
inline u64 binary(const char* data, std::size_t size) {
    u64 value = 0;
    if (size > 64 || size == 0) throw std::runtime_error("invalid binary width");
    for (std::size_t i = 0; i < size; ++i) {
        if (data[i] != '0' && data[i] != '1') throw std::runtime_error("unexpected non-binary value");
        value = (value << 1) | (data[i] - '0');
    }
    return value;
}

struct Selection {
    Digest digest;
    const Query* query = nullptr;
    std::exception_ptr error;
    void signal(u64 time, unsigned id, u64 value) {
        if (!query || (id == query->source && time >= query->begin && time <= query->end && value == query->value))
            digest.signal(time, id, value);
    }
    void tx(const Transaction& t) {
        if (!query || (t.generator == query->source && t.begin <= query->end && t.end >= query->begin && t.attrs[1] == query->value))
            digest.tx(t);
    }
    template<class F> int capture(F&& f) noexcept {
        try { f(); return 0; }
        catch (...) { error = std::current_exception(); return 1; }
    }
    void rethrow() { if (error) std::rethrow_exception(error); }
};

class VtrReader {
    vtr::Reader reader_;
    std::map<std::uint32_t, unsigned> generators_;
    std::vector<vtr::GeneratorId> generator_ids_;

  public:
    VtrReader(const Profile& profile, const std::string& path)
        : reader_(path, {.verify_checksums = false}) {
        if (reader_.signals().size() != profile.signals) {
            throw std::runtime_error("VTR signal count mismatch");
        }
        for (unsigned i = 0; i < profile.generators; ++i) {
            auto id = reader_.find_generator("top.generator_" + std::to_string(i));
            if (!id) {
                throw std::runtime_error("missing VTR generator");
            }
            generator_ids_.push_back(*id);
            generators_[id->value] = i;
        }
    }

    void signals(Selection& selection) {
        std::optional<vtr::SignalId> signal;
        vtr::TimeWindow window;
        if (selection.query) {
            signal = vtr::SignalId{selection.query->source};
            window = {selection.query->begin, selection.query->end};
        }
        reader_.changes(signal, window, [&](const vtr::SignalChange& change) {
            selection.signal(change.time, change.signal.value, change.value.as_u64());
            return vtr::Flow::next;
        });
    }

    void point(Selection& selection, const Query& query) {
        selection.digest.signal(
            query.begin, query.source,
            reader_.value_at(vtr::SignalId{query.source}, query.begin).as_u64());
    }

    void transactions(Selection& selection) {
        vtr::TransactionQuery query;
        if (selection.query) {
            query.generator = generator_ids_.at(selection.query->source);
            query.time = {selection.query->begin, selection.query->end};
        }
        reader_.transactions(query, [&](const vtr::Record& record) {
            Transaction transaction{
                0, record.begin(), record.end(), generators_.at(record.generator()->value),
                record.payload().as<std::array<std::uint64_t, 5>>()};
            transaction.serial = transaction.attrs[0];
            selection.tx(transaction);
            return vtr::Flow::next;
        });
    }
};

inline void read_fst(const Profile& p, const std::string& path, Selection& select, bool point) {
    std::unique_ptr<fstReaderContext, decltype(&fstReaderClose)> reader(
        require(fstReaderOpen(path.c_str()), "cannot open FST"), fstReaderClose);
    if (fstReaderGetVarCount(reader.get()) != p.signals) throw std::runtime_error("FST signal count mismatch");
    const auto q = select.query;
    if (point) {
        char buffer[65];
        auto value = require(fstReaderGetValueFromHandleAtTime(reader.get(), q->begin, q->source + 1, buffer), "FST point lookup failed");
        select.digest.signal(q->begin, q->source, binary(value, width(q->source)));
        return;
    }
    if (q) {
        fstReaderClrFacProcessMaskAll(reader.get());
        fstReaderSetFacProcessMask(reader.get(), q->source + 1);
        fstReaderSetLimitTimeRange(reader.get(), q->begin, q->end);
    } else fstReaderSetFacProcessMaskAll(reader.get());
    auto callback = [](void* user, u64 time, fstHandle handle, const unsigned char* value) {
        auto& s = *static_cast<Selection*>(user);
        if (!s.error) s.capture([&] {
            s.signal(time, handle - 1, binary(reinterpret_cast<const char*>(value), width(handle - 1)));
        });
    };
    if (!fstReaderIterBlocks(reader.get(), callback, &select, nullptr)) throw std::runtime_error("FST iteration failed");
    select.rethrow();
}

inline void read_vcd(const Profile& p, const std::string& path, Selection& select, bool point) {
    std::vector<char> buffer(1024 * 1024);
    std::ifstream file;
    file.rdbuf()->pubsetbuf(buffer.data(), buffer.size());
    file.open(path);
    if (!file) throw std::runtime_error("cannot open VCD");
    std::string line;
    u64 time = 0, last = 0;
    bool found = false, definitions = false;
    unsigned variables = 0;
    while (std::getline(file, line)) {
        if (line.empty()) continue;
        if (!definitions) {
            if (line.rfind("$var ", 0) == 0) ++variables;
            if (line == "$enddefinitions $end") {
                definitions = true;
                if (variables != p.signals) throw std::runtime_error("VCD signal count mismatch");
            }
            continue;
        }
        if (line[0] == '#') {
            auto result = std::from_chars(line.data() + 1, line.data() + line.size(), time);
            if (result.ec != std::errc{} || result.ptr != line.data() + line.size()) throw std::runtime_error("invalid VCD time");
            if (select.query && time > (point ? select.query->begin : select.query->end)) break;
            continue;
        }
        std::size_t value_begin = 0, value_end = 1, id_begin = 1;
        if (line[0] == 'b') {
            value_begin = 1;
            value_end = line.find(' ');
            if (value_end == std::string::npos) throw std::runtime_error("invalid VCD vector");
            id_begin = value_end + 1;
        }
        u64 id = 0, multiplier = 1;
        if (id_begin == line.size()) throw std::runtime_error("missing VCD identifier");
        for (std::size_t i = id_begin; i < line.size(); ++i) {
            if (line[i] < '!' || line[i] > '~' || multiplier > 94 * 94) throw std::runtime_error("invalid VCD identifier");
            id += (line[i] - '!') * multiplier;
            multiplier *= 94;
        }
        if (id >= p.signals) throw std::runtime_error("unknown VCD identifier");
        if (select.query && id != select.query->source) continue;
        auto value = binary(line.data() + value_begin, value_end - value_begin);
        if (point) { last = value; found = true; }
        else select.signal(time, unsigned(id), value);
    }
    if (file.bad() || !definitions) throw std::runtime_error("VCD read failed");
    if (point) {
        if (!found) throw std::runtime_error("missing VCD initial value");
        select.digest.signal(select.query->begin, select.query->source, last);
    }
}

inline Digest read(const Profile& p, const std::string& format, const std::string& path,
                   const std::string& operation, unsigned query_index) {
    const bool wave = operation == "search-signal" || operation == "find-signal" || operation == "point";
    const bool tx = operation == "search-transaction" || operation == "find-transaction";
    const bool whole_trace = operation == "find-signal" || operation == "find-transaction";
    auto q = (wave || tx) ? query(p, query_index, tx, whole_trace) : Query{};
    Selection select;
    if (wave || tx) select.query = &q;
    if (format == "vtr") {
        VtrReader reader(p, path);
        if (operation == "point") reader.point(select, q);
        else if (wave) reader.signals(select);
        else if (tx) reader.transactions(select);
        else {
            if (p.signals) reader.signals(select);
            if (p.generators) reader.transactions(select);
        }
    } else {
        const bool paired = format.find('+') != std::string::npos;
        auto wave_path = paired ? path + ".wave" : path;
        auto tx_path = paired ? path + ".tx" : path;
        if (p.signals && !tx) {
            if (format.substr(0, 3) == "fst") read_fst(p, wave_path, select, operation == "point");
            else if (format.substr(0, 3) == "vcd") read_vcd(p, wave_path, select, operation == "point");
            else throw std::runtime_error("invalid signal reader");
        }
        if (p.generators && !wave) {
            FtrReader reader(tx_path);
            reader.visit(select.query, [&](const Transaction& t) { select.tx(t); });
        }
    }
    return select.digest;
}
} // namespace bench
