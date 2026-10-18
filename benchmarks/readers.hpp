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
    std::map<vtr::Id, unsigned> generators_, attrs_;
    std::vector<vtr::Id> generator_ids_;
public:
    VtrReader(const Profile& p, const std::string& path) : reader_(path) {
        if (reader_.signals().size() != p.signals) throw std::runtime_error("VTR signal count mismatch");
        for (unsigned g = 0; g < p.generators; ++g) {
            auto node = reader_.find_node("top.generator_" + std::to_string(g));
            if (node == vtr::none) throw std::runtime_error("missing VTR generator");
            generator_ids_.push_back(node);
            generators_[node] = g;
        }
        const auto strings = reader_.strings();
        for (vtr::Id i = 0; i < strings.size(); ++i) {
            const std::string_view name = strings[i];
            for (unsigned a = 0; a < attr_names.size(); ++a)
                if (name == attr_names[a]) attrs_[i] = a;
        }
    }
    void signals(Selection& select) {
        auto callback = [&](u64 time, vtr::Id signal, const vtr::SignalValue& value) {
            select.signal(time, signal, value.as_u64());
            return true;
        };
        if (select.query) {
            const auto& q = *select.query;
            reader_.changes(q.source, q.begin, q.end, callback);
        } else reader_.changes(0, unlimited, callback);
    }
    void point(Selection& select, const Query& q) {
        auto bytes = reader_.value_at(q.source, q.begin);
        vtr::SignalValue value{reader_.signals()[q.source], bytes};
        select.digest.signal(q.begin, q.source, value.as_u64());
    }
    void transactions(Selection& select) {
        vtr::Query query;
        if (select.query) {
            query.generator = generator_ids_.at(select.query->source);
            query.begin = select.query->begin;
            query.end = select.query->end;
        }
        reader_.transactions(query, [&](const vtr::Transaction& tx) {
            if (tx.attrs.size() != 5) throw std::runtime_error("VTR attribute count mismatch");
            Transaction t{0, tx.begin, tx.end, generators_.at(tx.generator), {}};
            unsigned seen = 0;
            for (const auto& attr : tx.attrs) {
                if (attr.value.tag != vtr::Tag::u64) throw std::runtime_error("unexpected VTR attribute type");
                auto slot = attrs_.at(attr.key);
                if (seen & (1u << slot)) throw std::runtime_error("duplicate VTR attribute");
                seen |= 1u << slot;
                t.attrs[slot] = attr.value.number;
            }
            if (seen != 31) throw std::runtime_error("incomplete VTR transaction");
            t.serial = t.attrs[0];
            select.tx(t);
            return true;
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
