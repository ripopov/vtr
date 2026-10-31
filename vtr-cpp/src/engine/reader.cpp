#include "containers.h"
#include "engine/vtr.h"
#include "format.h"
#include "transaction_memory.h"
#include <deque>
#include <list>
#include <map>

namespace vtr::detail::engine {
namespace {
namespace d = vtr::detail::wire::detail;
using d::require;

std::size_t owned_bytes(const Transaction& tx) {
    const auto indexes = detail::memory_add(tx.events.size(), tx.stages.size());
    return detail::memory_add(detail::owned_tx(tx), detail::memory_mul(indexes, 48));
}

class LocatedPageCache {
  public:
    using Key = std::pair<std::uint64_t, std::uint32_t>;

  private:
    struct Entry {
        Key key;
        std::shared_ptr<const Bytes> page;
    };

    using Queue = std::list<Entry>;
    const std::size_t limit_;
    std::size_t bytes_ = 0;
    Queue order_;
    std::map<Key, Queue::iterator> entries_;

  public:
    explicit LocatedPageCache(std::size_t limit) : limit_(limit) {}

    std::shared_ptr<const Bytes> get(Key key) const {
        const auto found = entries_.find(key);
        return found == entries_.end() ? nullptr : found->second->page;
    }

    void insert(Key key, const std::shared_ptr<const Bytes>& page) {
        const auto bytes = page->capacity();
        if (!limit_ || bytes > limit_ || entries_.contains(key))
            return;
        order_.push_back({key, page});
        try {
            entries_.emplace(key, std::prev(order_.end()));
        } catch (...) {
            order_.pop_back();
            throw;
        }
        while (bytes > limit_ - bytes_) {
            const auto& oldest = order_.front();
            bytes_ -= oldest.page->capacity();
            entries_.erase(oldest.key);
            order_.pop_front();
        }
        bytes_ += bytes;
    }

    std::size_t size() const {
        return entries_.size();
    }

    std::size_t bytes() const {
        return bytes_;
    }

    void clear() {
        entries_.clear();
        order_.clear();
        bytes_ = 0;
    }
};

Timestamp period(const Transaction& tx, const vtr::detail::wire::Reader& native) {
    std::optional<Timestamp> value;
    for (const auto& attr : tx.attributes) {
        if (native.string(attr.key) == "vtr.period") {
            require(attr.value.tag == Tag::time && attr.value.number, "invalid clock period");
            value = attr.value.number;
        }
    }
    if (tx.begin == tx.end) {
        require(!value, "single-edge clock stretch has a period");
        return 1;
    }
    require(value && (tx.end - tx.begin) % *value == 0, "clock stretch does not match period");
    return *value;
}

void label_attrs(std::span<const vtr::detail::wire::Attribute> attrs,
                 const vtr::detail::wire::Reader& native) {
    for (const auto& a : attrs) {
        if (native.string(a.key) == "vtr.label") {
            require(a.value.tag == Tag::str || a.value.tag == Tag::text,
                    "invalid label attribute type");
        }
    }
}
} // namespace

struct Reader::Impl {
    struct Detail {
        RecordId owner;
        std::size_t index;
        bool stage;
    };

    ContainerReader file;
    ReaderOptions options;
    mutable d::HashMap<RecordId, TransactionRef> cache;
    mutable d::HashMap<RecordId, Detail> details;
    mutable std::deque<RecordId> order;
    mutable std::size_t cache_bytes = 0;

    mutable LocatedPageCache located_pages;

    std::shared_ptr<const Bytes> located_page(std::uint64_t block, std::uint32_t page) const {
        const LocatedPageCache::Key key{block, page};
        if (auto found = located_pages.get(key))
            return found;
        auto raw = std::make_shared<const Bytes>(file.record_block(block)->read_page(page));
        located_pages.insert(key, raw);
        return raw;
    }

    Impl(const std::string& path, ReaderOptions opts)
        : file(path, opts.verify_checksums, opts.max_payload_bytes), options(opts),
          located_pages(opts.located_page_cache_bytes) {}

    void prime_owner(const Transaction& tx) const {
        if (options.owner_cache_entries && options.owner_cache_bytes && !cached(tx.id) &&
            owned_bytes(tx) <= options.owner_cache_bytes) {
            remember(tx);
        }
    }

    TransactionRef cached(RecordId id) const {
        auto i = cache.find(id);
        return i == cache.end() ? nullptr : i->second;
    }

    void forget_details(const Transaction& tx) const {
        const auto erase = [&](RecordId id) {
            auto entry = details.find(id);
            if (entry != details.end() && entry->second.owner == tx.id)
                details.erase(entry);
        };
        for (const auto& event : tx.events)
            erase(event.id);
        for (const auto& stage : tx.stages)
            erase(stage.id);
    }

    TransactionRef remember(const Transaction& tx) const {
        if (auto existing = cached(tx.id)) {
            return existing;
        }
        auto ref = std::make_shared<const Transaction>(tx);
        auto bytes = owned_bytes(*ref);
        if (!options.owner_cache_entries || bytes > options.owner_cache_bytes) {
            return ref;
        }
        order.push_back(tx.id);
        try {
            cache.emplace(tx.id, ref);
            for (std::size_t i = 0; i < tx.events.size(); ++i) {
                details.emplace(tx.events[i].id, Detail{tx.id, i, false});
            }
            for (std::size_t i = 0; i < tx.stages.size(); ++i) {
                details.emplace(tx.stages[i].id, Detail{tx.id, i, true});
            }
        } catch (...) {
            forget_details(tx);
            cache.erase(tx.id);
            order.pop_back();
            throw;
        }
        while (cache.size() > options.owner_cache_entries ||
               bytes > options.owner_cache_bytes - cache_bytes) {
            auto id = order.front();
            order.pop_front();
            auto old = cache.find(id);
            cache_bytes -= owned_bytes(*old->second);
            forget_details(*old->second);
            cache.erase(old);
        }
        cache_bytes += bytes;
        return ref;
    }
};

Reader::Reader(const std::string& path, ReaderOptions options)
    : impl_(std::make_unique<Impl>(path, options)) {}

Reader::~Reader() = default;

const TraceId& Reader::trace_id() const noexcept {
    return impl_->file.waveform().trace_id();
}

std::optional<TraceFingerprint> Reader::fingerprint() const noexcept {
    return impl_->file.waveform().fingerprint();
}

const Types& Reader::types() const {
    return impl_->file.types();
}

const vtr::detail::wire::Meta& Reader::metadata() const {
    return impl_->file.waveform().metadata();
}

std::span<const vtr::detail::wire::Node> Reader::nodes() const {
    return impl_->file.waveform().nodes();
}

std::span<const std::string> Reader::strings() const {
    return impl_->file.waveform().strings();
}

std::string_view Reader::string(Id id) const {
    return impl_->file.waveform().string(id);
}

std::span<const SignalKind> Reader::signals() const {
    return impl_->file.waveform().signals();
}

Id Reader::find_node(std::string_view path) const {
    return impl_->file.waveform().find_node(path);
}

Id Reader::find_generator(std::string_view path) const {
    return impl_->file.waveform().find_node(path, '.', vtr::detail::wire::Node::generator);
}

Id Reader::find_signal(std::string_view path) const {
    return impl_->file.waveform().find_signal(path);
}

const ContainerReader& Reader::container() const noexcept {
    return impl_->file;
}

bool Reader::incomplete() const {
    return impl_->file.waveform().stats().recovered;
}

void Reader::changes(Timestamp begin, Timestamp end,
                     const std::function<bool(Timestamp, Id, const SignalValue&)>& visit) const {
    impl_->file.waveform().changes(begin, end, visit);
}

void Reader::changes(Id signal, Timestamp begin, Timestamp end,
                     const std::function<bool(Timestamp, Id, const SignalValue&)>& visit) const {
    impl_->file.waveform().changes(signal, begin, end, visit);
}

Bytes Reader::value_at(Id signal, Timestamp time) const {
    return impl_->file.waveform().value_at(signal, time);
}

std::shared_ptr<const vtr::detail::wire::SignalData> Reader::load_signal(Id signal) const {
    return impl_->file.waveform().load_signal(signal);
}

void Reader::transactions(TransactionQuery query,
                          const std::function<bool(const Transaction&)>& visit) const {
    impl_->file.transactions(query, [&](const auto& tx) {
        if (!tx.child_blocks.empty())
            impl_->prime_owner(tx);
        return visit(tx);
    });
}

void Reader::records(RecordQuery query,
                     const std::function<bool(const LocatedRecord&)>& visit) const {
    impl_->file.records(query, visit);
}

void Reader::relations(RecordId id, bool incoming,
                       const std::function<bool(const Relation&)>& visit) const {
    impl_->file.relations(id, incoming, visit);
}

TransactionRef Reader::transaction(RecordId id) const {
    if (!id) {
        return {};
    }
    auto& p = *impl_;
    if (auto ref = p.cached(id)) {
        return ref;
    }
    TransactionRef result;
    for (const auto& section : p.file.sections()) {
        if (section.kind != 5) {
            continue;
        }
        const auto& block = *p.file.transaction_block(section.block_number);
        const auto& b = block.bounds();
        if (!b.transactions || id < b.min_id || id > b.max_id) {
            continue;
        }
        p.file.section_data(section);
        block.transactions({.id = id}, [&](const auto& tx) {
            result = p.remember(tx);
            return false;
        });
        if (result) {
            break;
        }
    }
    return result;
}

std::optional<RecordRef> Reader::lookup(RecordId id) const {
    if (!id) {
        return {};
    }
    auto& p = *impl_;
    {
        auto i = p.cache.find(id);
        if (i != p.cache.end()) {
            return RecordRef(i->second, p.file.generators()[i->second->generator].kind);
        }
        auto detail = p.details.find(id);
        if (detail != p.details.end()) {
            auto owner = p.cache.at(detail->second.owner);
            return RecordRef(RecordRef::Embedded{owner, detail->second.index},
                             detail->second.stage ? RecordKind::embedded_stage
                                                  : RecordKind::embedded_event);
        }
    }
    for (const auto& section : p.file.sections()) {
        if (section.kind != 9) {
            continue;
        }
        const auto& block = *p.file.record_block(section.block_number);
        if (id < block.min_id() || id > block.max_id()) {
            continue;
        }
        std::shared_ptr<const LocatedRecord> result;
        p.file.section_data(section);
        block.query({.generator = {}, .parent = {}, .id = id}, [&](const auto& record) {
            result = std::make_shared<const LocatedRecord>(record);
            return false;
        });
        if (result) {
            return RecordRef(result, p.file.generators()[result->generator].kind);
        }
    }
    if (auto owner = transaction(id)) {
        return RecordRef(owner, p.file.generators()[owner->generator].kind);
    }
    for (const auto& section : p.file.sections()) {
        if (section.kind != 5) {
            continue;
        }
        const auto& block = *p.file.transaction_block(section.block_number);
        const auto& bounds = block.bounds();
        if (!bounds.max_embedded || id < bounds.min_embedded || id > bounds.max_embedded) {
            continue;
        }
        p.file.section_data(section);
        TransactionRef owner;
        block.transactions({.embedded_id = id}, [&](const auto& tx) {
            owner = p.remember(tx);
            return false;
        });
        if (!owner) {
            continue;
        }
        for (std::size_t i = 0; i < owner->events.size(); ++i) {
            if (owner->events[i].id == id) {
                return RecordRef(RecordRef::Embedded{owner, i}, RecordKind::embedded_event);
            }
        }
        for (std::size_t i = 0; i < owner->stages.size(); ++i) {
            if (owner->stages[i].id == id) {
                return RecordRef(RecordRef::Embedded{owner, i}, RecordKind::embedded_stage);
            }
        }
    }
    return {};
}

Expansion Reader::expand(RecordId id) const {
    Expansion result;
    result.owner = transaction(id);
    if (!result.owner) {
        return result;
    }
    require(impl_->file.generators()[result.owner->generator].kind == RecordKind::ordinary,
            "only ordinary transactions own located activity");
    for (auto number : result.owner->child_blocks) {
        auto section = impl_->file.block(number);
        if (!section) {
            result.missing_blocks.push_back(number);
            continue;
        }
        require(section->kind == 9, "located child locator references wrong block kind");
        impl_->file.section_data(*section);
        auto previous = result.located.size();
        impl_->file.record_block(number)->query(
            {.generator = {}, .parent = id, .include_logs = false},
            [&](const auto& record) {
                result.located.push_back(record);
                return true;
            },
            [&](std::uint32_t page) { return impl_->located_page(number, page); });
        require(result.located.size() != previous, "located child block has no matching child");
    }
    std::sort(result.located.begin(), result.located.end(),
              [](const auto& a, const auto& b) { return a.id < b.id; });
    result.complete = result.missing_blocks.empty();
    return result;
}

std::vector<Transaction> Reader::children(RecordId parent) const {
    require(parent, "child query requires a nonzero parent");
    std::vector<Transaction> result;
    transactions({.parent = parent}, [&](const auto& tx) {
        result.push_back(tx);
        return true;
    });
    std::sort(result.begin(), result.end(),
              [](const auto& a, const auto& b) { return a.id < b.id; });
    return result;
}

std::vector<LocatedRecord> Reader::logs(RecordId parent, Timestamp begin, Timestamp end) const {
    std::vector<LocatedRecord> result;
    records(
        {.generator = {}, .parent = parent, .kind = RecordKind::log, .begin = begin, .end = end},
        [&](const auto& record) {
            result.push_back(record);
            return true;
        });
    std::sort(result.begin(), result.end(),
              [](const auto& a, const auto& b) { return a.id < b.id; });
    return result;
}

std::optional<std::uint64_t> Reader::cycle_at(GeneratorId gen, Timestamp time) const {
    require(impl_->file.generators()[gen].kind == RecordKind::clock,
            "cycle query requires a CLOCK generator");
    std::vector<Transaction> stretches;
    transactions({.generator = gen}, [&](const auto& tx) {
        stretches.push_back(tx);
        return true;
    });
    std::sort(stretches.begin(), stretches.end(),
              [](const auto& a, const auto& b) { return a.begin < b.begin; });
    std::optional<std::uint64_t> answer, last_cycle;
    std::optional<Timestamp> last_edge;
    for (const auto& stretch : stretches) {
        require(!last_edge || stretch.begin > *last_edge, "clock stretches overlap");
        auto step = period(stretch, impl_->file.waveform());
        auto count_minus_one = (stretch.end - stretch.begin) / step;
        require(!last_cycle || *last_cycle != UINT64_MAX, "cycle number exhaustion");
        auto first_cycle = last_cycle ? *last_cycle + 1 : 0;
        require(count_minus_one <= UINT64_MAX - first_cycle, "cycle number overflow");
        if (time >= stretch.begin) {
            answer = first_cycle + (std::min(time, stretch.end) - stretch.begin) / step;
        }
        last_cycle = first_cycle + count_minus_one;
        last_edge = stretch.end;
    }
    return answer;
}

std::optional<GeneratorId> Reader::clock_for(GeneratorId source) const {
    impl_->file.generators()[source];
    for (const auto& attr : nodes()[source].attrs) {
        if (string(attr.key) == "vtr.clock") {
            require(attr.value.tag == Tag::u32, "clock association must be a u32 generator ID");
            auto id = attr.value.number;
            if (id >= nodes().size()) {
                require(incomplete(), "missing associated clock generator");
                return {};
            }
            require(nodes()[id].kind == vtr::detail::wire::Node::generator &&
                        record_kind(string(nodes()[id].aux)) == RecordKind::clock,
                    "clock association references wrong node kind");
            return static_cast<GeneratorId>(id);
        }
    }
    return {};
}

ValidationReport Reader::validate() const {
    auto& p = *impl_;
    const auto& native = p.file.waveform();
    ValidationReport report;
    report.incomplete_file = incomplete();

    struct Info {
        RecordKind kind;
        RecordId parent;
        std::uint64_t block;
    };

    d::HashMap<RecordId, Info> records;
    d::HashMap<RecordId, std::vector<std::uint64_t>> declared, actual;

    struct Edge {
        Relation relation;
        std::uint64_t block;
    };

    std::vector<Edge> edges;
    d::HashMap<GeneratorId, std::vector<std::pair<Timestamp, Timestamp>>> clocks;
    auto add = [&](RecordId id, RecordKind kind, RecordId parent, std::uint64_t block) {
        require(id && records.emplace(id, Info{kind, parent, block}).second,
                "duplicate global record ID");
        ++report.records;
    };
    label_attrs(metadata().attrs, native);
    for (TypeId id = first_declared_type; id < types().size(); ++id)
        label_attrs(types().attributes(id), native);
    for (std::size_t i = 0; i < nodes().size(); ++i) {
        const auto& node = nodes()[i];
        label_attrs(node.attrs, native);
        if (node.kind != vtr::detail::wire::Node::generator) {
            continue;
        }
        const auto& schema = types()[node.payload_type];
        for (const auto& field : schema.fields) {
            if (field.name == "vtr.label") {
                require(field.type == 12 || field.type == 13, "invalid payload label datatype");
            }
        }
        for (const auto& attr : node.attrs) {
            auto key = string(attr.key);
            if (key == "vtr.clock") {
                require(attr.value.tag == Tag::u32, "clock association must be u32");
                auto target = attr.value.number;
                if (target >= nodes().size()) {
                    require(report.incomplete_file, "missing associated clock generator");
                    report.missing.push_back(
                        {MissingReference::Kind::clock_generator, i, target, 0});
                } else {
                    require(nodes()[target].kind == vtr::detail::wire::Node::generator &&
                                record_kind(string(nodes()[target].aux)) == RecordKind::clock,
                            "invalid associated clock generator");
                }
            }
        }
    }
    for (const auto& section : p.file.sections()) {
        p.file.verify_section(section);
        if (section.kind == 9) {
            p.file.record_block(section.block_number)->query({}, [&](const auto& record) {
                auto kind = p.file.generators()[record.generator].kind;
                add(record.id, kind, record.parent, section.block_number);
                if (kind == RecordKind::event || kind == RecordKind::stage) {
                    actual[record.parent].push_back(section.block_number);
                }
                return true;
            });
        }
        if (section.kind == 5) {
            p.file.transaction_block(section.block_number)
                ->scan(
                    [&](const auto& tx) {
                        auto kind = p.file.generators()[tx.generator].kind;
                        add(tx.id, kind, tx.parent, section.block_number);
                        label_attrs(tx.attributes, native);
                        const auto& schema =
                            types()[p.file.generators()[tx.generator].payload_type];
                        for (const auto& attr : tx.attributes) {
                            for (const auto& field : schema.fields) {
                                require(field.name != string(attr.key),
                                        "optional attribute duplicates payload field");
                            }
                        }
                        declared[tx.id] = tx.child_blocks;
                        for (const auto& e : tx.events) {
                            add(e.id, RecordKind::embedded_event, tx.id, section.block_number);
                            label_attrs(e.attributes, native);
                        }
                        for (const auto& s : tx.stages) {
                            add(s.id, RecordKind::embedded_stage, tx.id, section.block_number);
                            label_attrs(s.attributes, native);
                        }
                        if (kind == RecordKind::clock) {
                            period(tx, native);
                            clocks[tx.generator].push_back({tx.begin, tx.end});
                        }
                        return true;
                    },
                    [&](const auto& relation) {
                        label_attrs(relation.attributes, native);
                        edges.push_back({relation, section.block_number});
                        ++report.relations;
                        return true;
                    });
        }
    }
    auto missing = [&](MissingReference::Kind kind, RecordId source, RecordId target,
                       std::uint64_t block) {
        require(report.incomplete_file, "unresolved reference in normally finalized file");
        report.missing.push_back({kind, source, target, block});
    };
    native.validate_waveforms();
    for (const auto& [id, info] : records) {
        if (info.parent) {
            auto parent = records.find(info.parent);
            if (parent == records.end()) {
                missing(MissingReference::Kind::parent, id, info.parent, info.block);
            } else {
                require(parent->second.kind == RecordKind::ordinary && info.parent < id,
                        "invalid ownership parent kind/order");
            }
        }
    }
    for (const auto& edge : edges) {
        if (edge.relation.source && !records.contains(edge.relation.source)) {
            missing(MissingReference::Kind::relation_source, 0, edge.relation.source, edge.block);
        }
        if (edge.relation.destination && !records.contains(edge.relation.destination)) {
            missing(MissingReference::Kind::relation_destination, 0, edge.relation.destination,
                    edge.block);
        }
    }
    for (auto& [owner, blocks] : actual) {
        std::sort(blocks.begin(), blocks.end());
        blocks.erase(std::unique(blocks.begin(), blocks.end()), blocks.end());
        auto listed = declared.find(owner);
        if (listed == declared.end()) {
            continue;
        }
        for (auto block : blocks) {
            require(std::binary_search(listed->second.begin(), listed->second.end(), block),
                    "located child missing from owner's locator list");
        }
    }
    for (const auto& [owner, blocks] : declared) {
        for (auto number : blocks) {
            auto block = p.file.block(number);
            if (!block) {
                missing(MissingReference::Kind::child_block, owner, number,
                        records.at(owner).block);
                continue;
            }
            require(block->kind == 9, "child locator references wrong block kind");
            auto found = actual.find(owner);
            require(found != actual.end() &&
                        std::binary_search(found->second.begin(), found->second.end(), number),
                    "child locator has no matching located child");
        }
    }
    for (auto& [clock, stretches] : clocks) {
        (void)clock;
        std::sort(stretches.begin(), stretches.end());
        for (std::size_t i = 1; i < stretches.size(); ++i) {
            require(stretches[i].first > stretches[i - 1].second, "clock stretches share an edge");
        }
    }
    return report;
}

CacheStats Reader::cache_stats() const {
    auto& p = *impl_;
    return {p.cache.size(), p.cache_bytes, p.located_pages.size(), p.located_pages.bytes()};
}

void Reader::clear_cache() const {
    auto& p = *impl_;
    {
        p.cache.clear();
        p.details.clear();
        p.order.clear();
        p.cache_bytes = 0;
        p.located_pages.clear();
    }
    p.file.waveform().clear_cache();
}

TransactionRef RecordRef::transaction() const {
    auto p = std::get_if<TransactionRef>(&data_);
    return p ? *p : nullptr;
}

const LocatedRecord* RecordRef::located() const {
    auto p = std::get_if<std::shared_ptr<const LocatedRecord>>(&data_);
    return p ? p->get() : nullptr;
}

const EmbeddedEvent* RecordRef::event() const {
    auto p = std::get_if<Embedded>(&data_);
    return p && kind_ == RecordKind::embedded_event ? &p->owner->events[p->index] : nullptr;
}

const EmbeddedStage* RecordRef::stage() const {
    auto p = std::get_if<Embedded>(&data_);
    return p && kind_ == RecordKind::embedded_stage ? &p->owner->stages[p->index] : nullptr;
}

RecordId RecordRef::id() const {
    if (auto p = transaction()) {
        return p->id;
    }
    if (auto p = located()) {
        return p->id;
    }
    if (auto p = event()) {
        return p->id;
    }
    return stage()->id;
}

RecordId RecordRef::parent() const {
    if (auto p = transaction()) {
        return p->parent;
    }
    if (auto p = located()) {
        return p->parent;
    }
    return std::get<Embedded>(data_).owner->id;
}

std::optional<GeneratorId> RecordRef::generator() const {
    if (auto p = transaction()) {
        return p->generator;
    }
    if (auto p = located()) {
        return p->generator;
    }
    return {};
}

Timestamp RecordRef::begin() const {
    if (auto p = transaction()) {
        return p->begin;
    }
    if (auto p = located()) {
        return p->begin;
    }
    if (auto p = event()) {
        return p->time;
    }
    return stage()->begin;
}

Timestamp RecordRef::end() const {
    if (auto p = transaction()) {
        return p->end;
    }
    if (auto p = located()) {
        return p->end;
    }
    if (auto p = event()) {
        return p->time;
    }
    auto p = stage();
    return p->end.value_or(std::max(p->begin, std::get<Embedded>(data_).owner->end));
}

std::shared_ptr<const void> RecordRef::retained_storage() const {
    return std::visit(
        []<class T>(const T& value) -> std::shared_ptr<const void> {
            if constexpr (std::same_as<T, Embedded>)
                return value.owner;
            else
                return value;
        },
        data_);
}

View RecordRef::payload() const {
    if (auto p = transaction()) {
        return p->payload;
    }
    if (auto p = located()) {
        return p->payload;
    }
    return {};
}

std::span<const vtr::detail::wire::Attribute> RecordRef::attributes() const {
    if (auto p = transaction()) {
        return p->attributes;
    }
    if (auto p = event()) {
        return p->attributes;
    }
    if (auto p = stage()) {
        return p->attributes;
    }
    return {};
}
} // namespace vtr::detail::engine
