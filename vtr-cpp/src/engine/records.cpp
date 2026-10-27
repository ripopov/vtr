#include "engine/records.h"
#include "format.h"
#include <numeric>
#include <unordered_set>

namespace vtr::detail::engine {
namespace {
namespace d = vtr::detail::wire::detail;
using d::Cursor;
using d::require;

std::uint32_t u32(std::size_t n) {
    require(n <= UINT32_MAX, "record block field exceeds u32");
    return static_cast<std::uint32_t>(n);
}

std::size_t var_size(std::uint64_t n) {
    return n ? (64 - std::countl_zero(n) + 6) / 7 : 1;
}

struct SegmentEncoder {
    std::array<Bytes, 4> columns;
    ArgumentColumns arguments;
    RecordId previous_id = 0, previous_parent = 0;
    Timestamp previous_end = 0;
    std::size_t dictionary_bytes = 0;

    void append(const Types& types, const Generator& generator, const LocatedRecord& record,
                TypeId datatype) {
        bool stage = generator.kind == RecordKind::stage;
        d::sint(columns[0], record.begin - previous_end);
        d::sint(columns[1], record.id - previous_id);
        if (generator.kind == RecordKind::log) {
            d::put(columns[2], record.parent ? record.id - record.parent : 0);
        } else {
            d::sint(columns[2], record.parent - previous_parent);
        }
        if (stage) {
            d::put(columns[3], record.end - record.begin);
        }
        auto old_dictionary = arguments.dictionary.size();
        if (datatype == 0)
            require(record.payload.empty(), "trailing unit payload");
        else
            arguments.append(types, datatype, record.payload);
        for (auto i = old_dictionary; i < arguments.dictionary.size(); ++i) {
            dictionary_bytes +=
                var_size(arguments.dictionary[i].size()) + arguments.dictionary[i].size();
        }
        previous_id = record.id;
        previous_parent = record.parent;
        previous_end = stage ? record.end : record.begin;
    }

    std::size_t size() const {
        std::size_t total = 1;
        for (const auto& column : columns) {
            total += var_size(column.size()) + column.size();
        }
        for (unsigned i = 0; i < 5; ++i) {
            auto n = i == 3 ? var_size(std::uint64_t(arguments.dictionary.size()) << 1) +
                                  dictionary_bytes
                            : arguments.columns[i].size();
            total += var_size(n) + n;
        }
        return total;
    }

    Bytes finish() {
        arguments.finish();
        std::array<Bytes, 9> all;
        for (unsigned i = 0; i < 4; ++i) {
            all[i] = std::move(columns[i]);
        }
        for (unsigned i = 0; i < 5; ++i) {
            all[i + 4] = std::move(arguments.columns[i]);
        }
        return d::pack_columns(all);
    }
};

struct Bounds {
    RecordId min_id = UINT64_MAX, max_id = 0, min_parent = UINT64_MAX, max_parent = 0;
    Timestamp begin = UINT64_MAX, end = 0;
    std::uint64_t count = 0;

    void add(const LocatedRecord& record) {
        ++count;
        min_id = std::min(min_id, record.id);
        max_id = std::max(max_id, record.id);
        if (record.parent) {
            min_parent = std::min(min_parent, record.parent);
            max_parent = std::max(max_parent, record.parent);
        }
        begin = std::min(begin, record.begin);
        end = std::max(end, record.end);
    }
};

void valid_record(const LocatedRecord& record, const Generator& generator) {
    require(record.id && record.parent < record.id, "invalid record identity/parent");
    require((generator.kind == RecordKind::log) == record.datatype().has_value(),
            "record datatype does not match source kind");
    require(generator.kind >= RecordKind::event && generator.kind <= RecordKind::log,
            "invalid located generator kind");
    require(generator.kind == RecordKind::log || record.parent,
            "located activity requires an owner");
    require(record.end >= record.begin, "negative stage duration");
    require(generator.kind == RecordKind::stage || record.begin == record.end,
            "point record has duration");
}
} // namespace

void Generators::declare(Generator generator) {
    require(generator.kind <= RecordKind::log, "unknown generator kind");
    require((generator.kind != RecordKind::clock && generator.kind != RecordKind::log) ||
                generator.payload_type == 0,
            "clock and logger generators require unit datatype");
    require(generator.id != UINT32_MAX, "invalid generator identifier");
    require(generators_.emplace(generator.id, generator).second, "generator redeclared");
    if (generator.id < prefix_.size())
        prefix_[generator.id] = generator;
}

TypeId Generators::payload_type(GeneratorId id, std::optional<std::uint32_t> datatype) const {
    const auto& generator = (*this)[id];
    if (generator.kind == RecordKind::log) {
        require(datatype.has_value(), "log requires a datatype");
        return *datatype;
    }
    require(!datatype, "fixed-schema record has a datatype");
    return generator.payload_type;
}

const Generator& Generators::lookup(GeneratorId id) const {
    auto i = generators_.find(id);
    require(i != generators_.end(), "undeclared generator");
    return i->second;
}

Bytes encode_record_block(std::uint64_t number, const Types& types, const Generators& generators,
                          std::span<const LocatedRecord> records, RecordPolicy policy,
                          std::uint64_t string_count) {
    require(number && !records.empty() && policy.page_bytes &&
                policy.codec <= vtr::detail::wire::Codec::zstd,
            "empty record block or invalid policy");

    struct Bucket {
        const Generator* generator;
        std::optional<std::uint32_t> datatype;
        TypeId payload_type;
        std::vector<const LocatedRecord*> records;
    };

    std::vector<Bucket> buckets;
    std::unordered_map<std::uint64_t, std::size_t> by_generator;
    for (const auto& record : records) {
        const auto& generator = generators[record.generator];
        valid_record(record, generator);
        auto datatype = record.datatype().value_or(generator.payload_type);
        auto key = (std::uint64_t(record.generator) << 32) | record.datatype().value_or(0);
        auto [i, fresh] = by_generator.try_emplace(key, buckets.size());
        if (fresh) {
            buckets.push_back({&generator, record.datatype(), datatype, {}});
        }
        buckets[i->second].records.push_back(&record);
    }
    if (policy.separate_logs) {
        std::stable_partition(buckets.begin(), buckets.end(), [](const Bucket& bucket) {
            return bucket.generator->kind != RecordKind::log;
        });
    }
    std::vector<SegmentHeader> segments;
    std::vector<PageHeader> pages;
    Bytes raw_page, blobs;
    auto flush_page = [&] {
        if (raw_page.empty()) {
            return;
        }
        auto packed = d::compress(raw_page, policy.codec, policy.compression_level);
        pages.push_back({u32(blobs.size()), u32(packed.size()), u32(raw_page.size())});
        d::append(blobs, packed);
        raw_page.clear();
    };
    Bounds all;
    bool had_activity = false;
    for (const auto& bucket : buckets) {
        bool log = bucket.generator->kind == RecordKind::log;
        if (policy.separate_logs && log && had_activity) {
            flush_page();
            had_activity = false;
        } else if (!log) {
            had_activity = true;
        }
        std::size_t first = 0;
        while (first < bucket.records.size()) {
            SegmentEncoder encoder;
            encoder.arguments.string_count = string_count;
            Bounds bounds;
            auto index = first;
            auto measured_size = encoder.size();
            std::size_t unmeasured_upper = 0;
            for (; index < bucket.records.size(); ++index) {
                const auto& record = *bucket.records[index];
                // Four row varints and nine growing column-length prefixes fit in 128 bytes.
                const auto upper = record.payload.size() > (SIZE_MAX - 128) / 10
                                       ? SIZE_MAX
                                       : 128 + record.payload.size() * 10;
                auto remaining = [&] {
                    return policy.page_bytes - std::min(measured_size, policy.page_bytes);
                };
                if (policy.defer_page_size_scans && unmeasured_upper &&
                    (unmeasured_upper >= remaining() || upper >= remaining() - unmeasured_upper)) {
                    measured_size = encoder.size();
                    unmeasured_upper = 0;
                }
                const bool bounded = policy.defer_page_size_scans &&
                                     unmeasured_upper < remaining() &&
                                     upper < remaining() - unmeasured_upper;
                std::array<std::size_t, 9> sizes;
                std::size_t dictionary_count = 0, dictionary_bytes = 0;
                if (!bounded && index != first) {
                    for (unsigned c = 0; c < 4; ++c)
                        sizes[c] = encoder.columns[c].size();
                    for (unsigned c = 0; c < 5; ++c)
                        sizes[c + 4] = encoder.arguments.columns[c].size();
                    dictionary_count = encoder.arguments.dictionary.size();
                    dictionary_bytes = encoder.dictionary_bytes;
                }
                encoder.append(types, *bucket.generator, record, bucket.payload_type);
                if (bounded) {
                    unmeasured_upper += upper;
                } else {
                    measured_size = encoder.size();
                    unmeasured_upper = 0;
                    if (index != first && measured_size > policy.page_bytes) {
                        for (unsigned c = 0; c < 4; ++c)
                            encoder.columns[c].resize(sizes[c]);
                        for (unsigned c = 0; c < 5; ++c)
                            encoder.arguments.columns[c].resize(sizes[c + 4]);
                        while (encoder.arguments.dictionary.size() > dictionary_count) {
                            encoder.arguments.text_ids.erase(encoder.arguments.dictionary.back());
                            encoder.arguments.dictionary.pop_back();
                        }
                        encoder.dictionary_bytes = dictionary_bytes;
                        break;
                    }
                }
                bounds.add(record);
                all.add(record);
                if (measured_size >= policy.page_bytes) {
                    ++index;
                    break;
                }
            }
            auto raw = encoder.finish();
            if (!raw_page.empty() &&
                raw.size() > policy.page_bytes - std::min(raw_page.size(), policy.page_bytes)) {
                flush_page();
            }
            auto offset = u32(raw_page.size());
            d::append(raw_page, raw);
            segments.push_back({bucket.generator->id, u32(pages.size()), u32(index - first), offset,
                                u32(raw.size()), bounds.min_id, bounds.max_id,
                                bounds.max_parent ? bounds.min_parent : 0, bounds.max_parent,
                                bounds.begin, bounds.end, bucket.datatype});
            if (raw_page.size() >= policy.page_bytes) {
                flush_page();
            }
            first = index;
        }
    }
    flush_page();
    Bytes payload;
    for (auto value : {number, all.count, all.min_id, all.max_id, all.begin, all.end}) {
        d::fixed(payload, value);
    }
    d::fixed(payload, u32(segments.size()));
    d::fixed(payload, u32(pages.size()));
    for (const auto& s : segments) {
        d::fixed(payload, s.generator);
        if (s.datatype)
            d::fixed(payload, *s.datatype);
        for (auto v : {s.page, s.count, s.raw_offset, s.raw_length}) {
            d::fixed(payload, v);
        }
        for (auto v : {s.min_id, s.max_id, s.min_parent, s.max_parent, s.begin, s.end}) {
            d::fixed(payload, v);
        }
    }
    for (const auto& page : pages) {
        d::fixed(payload, page.blob_offset);
        d::fixed(payload, page.blob_length);
        d::fixed(payload, page.raw_length);
    }
    d::append(payload, blobs);
    return payload;
}

RecordBlockView::RecordBlockView(View encoded, const Types& types, const Generators& generators,
                                 std::uint64_t string_count, std::size_t max_payload_bytes)
    : encoded_(encoded), types_(types), generators_(generators), string_count_(string_count),
      max_payload_bytes_(max_payload_bytes) {
    Cursor r(encoded);
    number_ = r.fixed<std::uint64_t>();
    count_ = r.fixed<std::uint64_t>();
    min_id_ = r.fixed<std::uint64_t>();
    max_id_ = r.fixed<std::uint64_t>();
    begin_ = r.fixed<std::uint64_t>();
    end_ = r.fixed<std::uint64_t>();
    require(number_ && count_ && min_id_ && min_id_ <= max_id_ && begin_ <= end_,
            "invalid record block bounds");
    auto segment_count = r.fixed<std::uint32_t>(), page_count = r.fixed<std::uint32_t>();
    require(segment_count && page_count && segment_count <= r.data.size() / 68,
            "invalid record segment count");
    for (std::uint32_t i = 0; i < segment_count; ++i) {
        SegmentHeader s;
        s.generator = r.fixed<std::uint32_t>();
        const auto& generator = generators_[s.generator];
        if (generator.kind == RecordKind::log)
            s.datatype = r.fixed<std::uint32_t>();
        s.page = r.fixed<std::uint32_t>();
        s.count = r.fixed<std::uint32_t>();
        s.raw_offset = r.fixed<std::uint32_t>();
        s.raw_length = r.fixed<std::uint32_t>();
        s.min_id = r.fixed<std::uint64_t>();
        s.max_id = r.fixed<std::uint64_t>();
        s.min_parent = r.fixed<std::uint64_t>();
        s.max_parent = r.fixed<std::uint64_t>();
        s.begin = r.fixed<std::uint64_t>();
        s.end = r.fixed<std::uint64_t>();
        types_[generators_.payload_type(s.generator, s.datatype)];
        require(generator.kind >= RecordKind::event && generator.kind <= RecordKind::log &&
                    s.page < page_count && s.count && s.raw_length,
                "invalid record segment");
        require(s.min_id && s.min_id <= s.max_id && s.min_parent <= s.max_parent &&
                    (s.min_parent != 0 || s.max_parent == 0) && s.begin <= s.end &&
                    (generator.kind == RecordKind::log || s.min_parent != 0),
                "invalid record segment bounds");
        segments_.push_back(s);
    }
    require(page_count <= r.data.size() / 12, "invalid record page count");
    std::uint64_t blob_end = 0;
    for (std::uint32_t i = 0; i < page_count; ++i) {
        PageHeader p{r.fixed<std::uint32_t>(), r.fixed<std::uint32_t>(), r.fixed<std::uint32_t>()};
        require(p.blob_length && p.raw_length && p.blob_offset == blob_end,
                "invalid record page partition");
        blob_end += p.blob_length;
        pages_.push_back(p);
    }
    blobs_ = r.take(blob_end);
    r.done();
    std::uint64_t count = 0;
    std::size_t page = 0;
    std::uint64_t raw_offset = 0;
    for (const auto& s : segments_) {
        if (s.page != page) {
            require(s.page == page + 1 && raw_offset == pages_[page].raw_length,
                    "invalid record segment partition");
            ++page;
            raw_offset = 0;
        }
        require(s.raw_offset == raw_offset, "invalid record segment offset");
        raw_offset += s.raw_length;
        require(raw_offset <= pages_[page].raw_length, "record segment exceeds page");
        count += s.count;
    }
    require(page + 1 == pages_.size() && raw_offset == pages_[page].raw_length && count == count_,
            "record segment counts/partition mismatch");
    RecordId min_id = UINT64_MAX, max_id = 0;
    Timestamp begin = UINT64_MAX, end = 0;
    for (const auto& s : segments_) {
        min_id = std::min(min_id, s.min_id);
        max_id = std::max(max_id, s.max_id);
        begin = std::min(begin, s.begin);
        end = std::max(end, s.end);
    }
    require(min_id == min_id_ && max_id == max_id_ && begin == begin_ && end == end_,
            "record header aggregate bounds mismatch");
}

void RecordBlockView::query(RecordQuery query,
                            const std::function<bool(const LocatedRecord&)>& visit) const {
    require(query.begin <= query.end, "invalid record query window");
    std::size_t segment_index = 0;
    for (std::size_t page_index = 0; page_index < pages_.size(); ++page_index) {
        Bytes raw;
        while (segment_index < segments_.size() && segments_[segment_index].page == page_index) {
            const auto& segment = segments_[segment_index++];
            const auto& generator = generators_[segment.generator];
            const auto datatype = segment.datatype.value_or(generator.payload_type);
            if ((query.datatype && *query.datatype != datatype) ||
                (query.generator && *query.generator != segment.generator) ||
                (!query.include_logs && generator.kind == RecordKind::log) ||
                (query.kind && *query.kind != generator.kind) ||
                (query.id && (*query.id < segment.min_id || *query.id > segment.max_id)) ||
                (query.parent && *query.parent &&
                 (*query.parent < segment.min_parent || *query.parent > segment.max_parent)) ||
                query.end < segment.begin || query.begin > segment.end) {
                continue;
            }
            const auto& page = pages_[page_index];
            if (raw.empty()) {
                raw = d::decompress(blobs_.subspan(page.blob_offset, page.blob_length));
                require(raw.size() == page.raw_length, "record page raw length mismatch");
            }
            auto columns =
                d::unpack_columns<9>(View(raw).subspan(segment.raw_offset, segment.raw_length));
            std::array<View, 5> argument_columns;
            for (unsigned i = 0; i < 5; ++i) {
                argument_columns[i] = columns[i + 4].data;
            }
            ArgumentReader args(argument_columns, string_count_, max_payload_bytes_);
            RecordId id = 0, parent = 0;
            Timestamp previous_end = 0;
            Bounds bounds;
            for (std::uint32_t i = 0; i < segment.count; ++i) {
                LocatedRecord record;
                record.generator = segment.generator;
                record.begin = previous_end + columns[0].signed_bits();
                record.id = id + columns[1].signed_bits();
                if (generator.kind == RecordKind::log) {
                    auto distance = columns[2].var();
                    require(!distance || distance < record.id, "invalid log parent distance");
                    record.parent = distance ? record.id - distance : 0;
                } else {
                    record.parent = parent + columns[2].signed_bits();
                }
                auto duration = generator.kind == RecordKind::stage ? columns[3].var() : 0;
                require(duration <= UINT64_MAX - record.begin, "stage end overflow");
                record.end = record.begin + duration;
                record.datatype_id = segment.datatype.value_or(UINT32_MAX);
                record.payload = args.read(types_, datatype);
                valid_record(record, generator);
                bounds.add(record);
                id = record.id;
                parent = record.parent;
                previous_end = record.end;
                bool point = record.begin == record.end;
                bool match = record.begin <= query.end &&
                             (point ? record.begin >= query.begin : record.end > query.begin);
                if (match && (!query.parent || *query.parent == record.parent) &&
                    (!query.id || *query.id == record.id) && !visit(record)) {
                    return;
                }
            }
            for (unsigned i = 0; i < 4; ++i) {
                columns[i].done();
            }
            args.done();
            require(bounds.min_id == segment.min_id && bounds.max_id == segment.max_id &&
                        bounds.begin == segment.begin && bounds.end == segment.end &&
                        bounds.max_parent == segment.max_parent &&
                        (bounds.max_parent ? bounds.min_parent : 0) == segment.min_parent,
                    "record segment bounds mismatch");
        }
    }
}

void RecordBlockView::validate() const {
    Bounds bounds;
    std::unordered_set<RecordId> ids;
    query({}, [&](const auto& record) {
        require(ids.insert(record.id).second, "duplicate record identifier");
        bounds.add(record);
        return true;
    });
    require(bounds.count == count_ && bounds.min_id == min_id_ && bounds.max_id == max_id_ &&
                bounds.begin == begin_ && bounds.end == end_,
            "record block bounds mismatch");
}
} // namespace vtr::detail::engine
