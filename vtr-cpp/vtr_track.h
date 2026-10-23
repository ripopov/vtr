#pragma once

// Keyed transaction lifetimes for one logical producer. Storage follows the
// peak live population and is reused; the caller advances writer time explicitly.
// Close or abort every item, or detach, before destruction to publish attributes.

#include "vtr_writer.h"

#include <algorithm>
#include <array>
#include <map>
#include <string>
#include <vector>

namespace vtr {

class Tracker final {
  public:
    class Keyspace {
      private:
        friend class Tracker;
        Keyspace(const Tracker* origin, std::uint32_t index) : origin_(origin), index_(index) {}
        const Tracker* origin_;
        std::uint32_t index_;
    };

    Tracker(Writer& writer, Source<Unit, SourceKind::transaction> source, std::string name)
        : writer_(writer), source_(source), name_(std::move(name)),
          label_(writer.intern("vtr.label")), empty_(writer.intern("")) {}

    Tracker(const Tracker&) = delete;
    Tracker& operator=(const Tracker&) = delete;

    [[nodiscard]] Keyspace keyspace(std::string name) {
        if (spaces_.size() == UINT32_MAX)
            throw Error(ErrorCode::resource_limit, "Too many tracker key spaces");
        spaces_.emplace_back(std::make_unique<Space>(std::move(name)));
        return {this, static_cast<std::uint32_t>(spaces_.size() - 1)};
    }

    [[nodiscard]] std::size_t open_count() const noexcept { return open_.size(); }
    [[nodiscard]] std::size_t keyspace_count() const noexcept { return spaces_.size(); }

    std::optional<RecordId> open(Keyspace space, std::uint64_t key, std::string_view stage,
                                 Timestamp time, std::optional<OwnerRef> parent = {}) {
        if (!valid(space, "open"))
            return {};
        // A replaced item may also be the new item's parent.
        const auto record = writer_.begin_at(source_, time, {.parent = parent});
        if (const Group* group = spaces_[space.index_]->keys.find(key)) {
            diag("open on a key that still names an item; that item is aborted", space.index_, key);
            snapshot(*group, doomed_);
            end_doomed(Status::aborted, time);
        }
        Item& item = allocate();
        item.record = record;
        item.sequence = ++sequence_;
        open_.push_back(&item);
        enter(item, empty_, intern(stage), time);
        name(item, space.index_, key);
        return record.id();
    }

    [[nodiscard]] std::optional<OwnerRef> owner(Keyspace space, std::uint64_t key) {
        const Group* group = lookup(space, key, "owner");
        return group ? std::optional<OwnerRef>(*group->at(0)->record) : std::nullopt;
    }

    [[nodiscard]] std::vector<RecordId> records(Keyspace space, std::uint64_t key) {
        std::vector<RecordId> result;
        if (const Group* group = lookup(space, key, "records"))
            for (std::size_t i = 0; i < group->size(); ++i)
                result.push_back(group->at(i)->record->id());
        return result;
    }

    void bind(Keyspace from, std::uint64_t key, Keyspace to, std::uint64_t target, Timestamp time) {
        if (!valid(to, "bind"))
            return;
        const Group* group = lookup(from, key, "bind");
        if (!group)
            return;
        snapshot(*group, moved_);
        evict(to.index_, target, time);
        for (Item* item : moved_)
            name(*item, to.index_, target);
    }

    std::size_t bind_oldest(Keyspace from, std::size_t count, Keyspace to, std::uint64_t target,
                            Timestamp time) {
        if (!valid(from, "bind_oldest") || !valid(to, "bind_oldest"))
            return 0;
        const auto& order = spaces_[from.index_]->order;
        const auto n = std::min(count, order.size());
        if (n != count)
            diag("bind_oldest found fewer items than requested", to.index_, target);
        moved_.assign(order.begin(), order.begin() + static_cast<std::ptrdiff_t>(n));
        evict(to.index_, target, time);
        for (Item* item : moved_) {
            unname(*item, from.index_);
            name(*item, to.index_, target);
        }
        return n;
    }

    void stage(Keyspace space, std::uint64_t key, std::string_view stage, Timestamp time) {
        if (const Group* group = lookup(space, key, "stage")) {
            const auto name = intern(stage);
            for (std::size_t i = 0; i < group->size(); ++i)
                enter(*group->at(i), empty_, name, time);
        }
    }

    void lane(Keyspace space, std::uint64_t key, std::string_view lane, std::string_view stage,
              Timestamp time) {
        if (const Group* group = lookup(space, key, "lane")) {
            const auto lane_id = intern(lane);
            const auto name = intern(stage);
            for (std::size_t i = 0; i < group->size(); ++i)
                enter(*group->at(i), lane_id, name, time);
        }
    }

    void event(Keyspace space, std::uint64_t key, std::string_view name, Timestamp time) {
        if (const Group* group = lookup(space, key, "event")) {
            const auto name_id = intern(name);
            for (std::size_t i = 0; i < group->size(); ++i)
                (void)writer_.embedded_event(*group->at(i)->record, time, name_id);
        }
    }

    void attribute(Keyspace space, std::uint64_t key, std::string_view name, std::uint64_t value) {
        if (const Group* group = lookup(space, key, "attribute"))
            set_attribute(*group, intern(name), Value(value));
    }

    void attribute(Keyspace space, std::uint64_t key, std::string_view name,
                   std::string_view text) {
        if (const Group* group = lookup(space, key, "attribute"))
            set_attribute(*group, intern(name), Value(intern(text)));
    }

    void label(Keyspace space, std::uint64_t key, std::string_view text) {
        if (const Group* group = lookup(space, key, "label"))
            set_attribute(*group, label_, Value(intern(text)));
    }

    std::size_t close(Keyspace space, std::uint64_t key, Status status, Timestamp time) {
        const Group* group = lookup(space, key, "close");
        if (!group)
            return 0;
        snapshot(*group, doomed_);
        return end_doomed(status, time);
    }

    std::size_t abort_younger(Keyspace space, std::uint64_t key, Timestamp time) {
        const Group* group = lookup(space, key, "abort_younger");
        if (!group)
            return 0;
        const auto last = group->at(group->size() - 1)->sequence;
        const auto begin =
            std::upper_bound(open_.begin(), open_.end(), last, [](auto sequence, const Item* item) {
                return sequence < item->sequence;
            });
        doomed_.assign(begin, open_.end());
        return end_doomed(Status::aborted, time);
    }

    std::size_t abort_keyspace(Keyspace space, Timestamp time) {
        if (!valid(space, "abort_keyspace"))
            return 0;
        doomed_ = spaces_[space.index_]->order;
        return end_doomed(Status::aborted, time);
    }

    std::size_t abort_all(Timestamp time) {
        doomed_ = open_;
        return end_doomed(Status::aborted, time);
    }

    void relate(std::string_view kind, Keyspace from, std::uint64_t key, Tracker& destination,
                Keyspace to, std::uint64_t target) {
        if (&writer_ != &destination.writer_)
            throw Error(ErrorCode::wrong_recording, "Trackers belong to different recordings");
        const Group* a = lookup(from, key, "relate");
        const Group* b = destination.lookup(to, target, "relate");
        if (!a || !b)
            return;
        const auto relation = intern(kind);
        for (std::size_t i = 0; i < a->size(); ++i)
            for (std::size_t j = 0; j < b->size(); ++j)
                writer_.relate(relation, a->at(i)->record->id(), b->at(j)->record->id());
    }

    // Detach preserves unfinished records in the writer; it does not fabricate ends.
    void detach() {
        for (Item* item : open_) {
            write_attributes(*item);
            release(*item);
        }
        open_.clear();
        for (auto& space : spaces_) {
            space->keys.clear();
            space->order.clear();
        }
    }

    struct Diagnostic {
        std::uint64_t count = 0;
        std::string first;
    };

    [[nodiscard]] const std::map<std::string, Diagnostic>& diagnostics() const noexcept {
        return diagnostics_;
    }

    std::vector<std::string> take_diagnostics() {
        std::vector<std::string> result;
        for (const auto& [reason, diagnostic] : diagnostics_)
            result.push_back(name_ + ": " + reason + " (" + std::to_string(diagnostic.count) +
                             (diagnostic.count == 1 ? " time" : " times") +
                             (diagnostic.first.empty() ? "" : ", first " + diagnostic.first) + ")");
        diagnostics_.clear();
        return result;
    }

  private:
    struct Lane {
        StringId name;
        StringId stage;
        std::optional<RecordId> visit;
    };

    struct Item {
        std::uint64_t sequence = 0;
        std::optional<Transaction<>> record;
        std::vector<std::pair<std::uint32_t, std::uint64_t>> names;
        std::vector<Lane> lanes;
        Attributes attributes;
    };

    struct Group {
        static constexpr std::size_t inline_size = 4;
        std::array<Item*, inline_size> first{};
        std::size_t count = 0;
        std::vector<Item*> more;

        [[nodiscard]] std::size_t size() const { return count + more.size(); }
        [[nodiscard]] Item* at(std::size_t index) const {
            return index < inline_size ? first[index] : more[index - inline_size];
        }

        void insert(Item* item) {
            std::size_t pos = size();
            while (pos && at(pos - 1)->sequence > item->sequence)
                --pos;
            if (count < inline_size) {
                std::move_backward(first.begin() + pos, first.begin() + count,
                                   first.begin() + count + 1);
                first[pos] = item;
                ++count;
            } else if (pos < inline_size) {
                more.insert(more.begin(), first.back());
                std::move_backward(first.begin() + pos, first.end() - 1, first.end());
                first[pos] = item;
            } else {
                more.insert(more.begin() + static_cast<std::ptrdiff_t>(pos - inline_size), item);
            }
        }

        void erase(Item* item) {
            for (std::size_t i = 0; i < count; ++i) {
                if (first[i] != item)
                    continue;
                std::move(first.begin() + i + 1, first.begin() + count, first.begin() + i);
                if (more.empty()) {
                    --count;
                } else {
                    first.back() = more.front();
                    more.erase(more.begin());
                }
                return;
            }
            std::erase(more, item);
        }

        void clear() {
            count = 0;
            more.clear();
        }
    };

    class KeyTable {
      public:
        const Group* find(std::uint64_t key) const {
            if (slots_.empty())
                return nullptr;
            for (std::size_t i = hash(key) & mask();; i = (i + 1) & mask()) {
                const Slot& slot = slots_[i];
                if (!slot.used)
                    return nullptr;
                if (slot.key == key)
                    return &slot.group;
            }
        }

        Group* find(std::uint64_t key) {
            return const_cast<Group*>(static_cast<const KeyTable*>(this)->find(key));
        }

        Group& insert(std::uint64_t key) {
            if ((count_ + 1) * 4 > slots_.size() * 3)
                grow();
            std::size_t index = hash(key) & mask();
            while (slots_[index].used && slots_[index].key != key)
                index = (index + 1) & mask();
            Slot& slot = slots_[index];
            if (!slot.used) {
                slot.used = true;
                slot.key = key;
                ++count_;
            }
            return slot.group;
        }

        void erase(std::uint64_t key) {
            if (slots_.empty())
                return;
            std::size_t i = hash(key) & mask();
            while (slots_[i].used && slots_[i].key != key)
                i = (i + 1) & mask();
            if (!slots_[i].used)
                return;
            --count_;
            for (std::size_t j = (i + 1) & mask(); slots_[j].used; j = (j + 1) & mask()) {
                const auto home = hash(slots_[j].key) & mask();
                if (((j - home) & mask()) >= ((j - i) & mask())) {
                    std::swap(slots_[i], slots_[j]);
                    i = j;
                }
            }
            slots_[i].used = false;
            slots_[i].group.clear();
        }

        void clear() {
            for (auto& slot : slots_) {
                slot.used = false;
                slot.group.clear();
            }
            count_ = 0;
        }

      private:
        struct Slot {
            std::uint64_t key = 0;
            bool used = false;
            Group group;
        };

        static std::size_t hash(std::uint64_t key) {
            key ^= key >> 33;
            key *= 0xff51afd7ed558ccdULL;
            key ^= key >> 33;
            return static_cast<std::size_t>(key);
        }
        std::size_t mask() const { return slots_.size() - 1; }

        void grow() {
            std::vector<Slot> previous;
            previous.swap(slots_);
            slots_.resize(previous.empty() ? 64 : previous.size() * 2);
            count_ = 0;
            for (auto& slot : previous)
                if (slot.used)
                    insert(slot.key) = std::move(slot.group);
        }
        std::vector<Slot> slots_;
        std::size_t count_ = 0;
    };

    struct Space {
        explicit Space(std::string text) : name(std::move(text)) {}
        std::string name;
        KeyTable keys;
        std::vector<Item*> order;
    };

    struct Intern {
        std::uint64_t hash = 0;
        std::optional<StringId> id;
        std::string text;
    };

    StringId intern(std::string_view text) {
        if (text.empty())
            return empty_;
        std::uint64_t hash = 1469598103934665603ULL;
        for (unsigned char value : text)
            hash = (hash ^ value) * 1099511628211ULL;
        auto& entry = strings_[hash & (strings_.size() - 1)];
        if (entry.id && entry.hash == hash && entry.text == text)
            return *entry.id;
        const auto id = writer_.intern(text);
        entry.text.assign(text);
        entry.hash = hash;
        entry.id = id;
        return *entry.id;
    }

    Item& allocate() {
        if (free_.empty()) {
            pool_.emplace_back(std::make_unique<Item>());
            return *pool_.back();
        }
        Item* item = free_.back();
        free_.pop_back();
        return *item;
    }

    void release(Item& item) {
        item.record.reset();
        item.names.clear();
        item.lanes.clear();
        item.attributes.clear();
        free_.push_back(&item);
    }

    bool valid(Keyspace space, std::string_view operation) {
        if (space.origin_ == this && space.index_ < spaces_.size())
            return true;
        diag(std::string(operation) + " with a key space of another tracker", UINT32_MAX, 0);
        return false;
    }

    const Group* lookup(Keyspace space, std::uint64_t key, std::string_view operation) {
        if (!valid(space, operation))
            return nullptr;
        const Group* group = spaces_[space.index_]->keys.find(key);
        if (!group)
            diag(std::string(operation) + " on a key that names no item", space.index_, key);
        return group;
    }

    static void snapshot(const Group& group, std::vector<Item*>& target) {
        target.clear();
        for (std::size_t i = 0; i < group.size(); ++i)
            target.push_back(group.at(i));
    }

    static void insert_ordered(std::vector<Item*>& items, Item& item) {
        const auto at = std::upper_bound(
            items.begin(), items.end(), item.sequence,
            [](auto sequence, const Item* other) { return sequence < other->sequence; });
        items.insert(at, &item);
    }

    static void erase_ordered(std::vector<Item*>& items, Item& item) {
        const auto at = std::lower_bound(
            items.begin(), items.end(), item.sequence,
            [](const Item* other, auto sequence) { return other->sequence < sequence; });
        if (at != items.end() && *at == &item)
            items.erase(at);
    }

    void name(Item& item, std::uint32_t space, std::uint64_t key) {
        for (const auto& previous : item.names) {
            if (previous.first != space)
                continue;
            if (previous.second == key)
                return;
            unname(item, space);
            break;
        }
        item.names.emplace_back(space, key);
        spaces_[space]->keys.insert(key).insert(&item);
        insert_ordered(spaces_[space]->order, item);
    }

    void unname(Item& item, std::uint32_t space) {
        for (auto at = item.names.begin(); at != item.names.end(); ++at) {
            if (at->first != space)
                continue;
            auto& target = *spaces_[space];
            if (Group* group = target.keys.find(at->second)) {
                group->erase(&item);
                if (!group->size())
                    target.keys.erase(at->second);
            }
            erase_ordered(target.order, item);
            item.names.erase(at);
            return;
        }
    }

    void evict(std::uint32_t space, std::uint64_t key, Timestamp time) {
        doomed_.clear();
        if (const Group* group = spaces_[space]->keys.find(key))
            for (std::size_t i = 0; i < group->size(); ++i)
                if (std::find(moved_.begin(), moved_.end(), group->at(i)) == moved_.end())
                    doomed_.push_back(group->at(i));
        if (!doomed_.empty()) {
            diag("bind to a key that still names another item; that item is aborted", space, key);
            end_doomed(Status::aborted, time);
        }
    }

    void enter(Item& item, StringId lane, StringId name, Timestamp time) {
        auto at = std::find_if(item.lanes.begin(), item.lanes.end(),
                               [&](const Lane& current) { return current.name == lane; });
        if (at != item.lanes.end()) {
            if (at->visit && at->stage == name && name != empty_)
                return;
            if (at->visit)
                writer_.end_embedded_stage(*item.record, *at->visit, time);
            at->visit.reset();
            if (name != empty_) {
                at->stage = name;
                at->visit = writer_.embedded_stage(*item.record, time, name, lane);
            }
        } else if (name != empty_) {
            item.lanes.push_back(
                {lane, name, writer_.embedded_stage(*item.record, time, name, lane)});
        }
    }

    static void set_attribute(const Group& group, StringId key, const Value& value) {
        for (std::size_t i = 0; i < group.size(); ++i) {
            auto& attributes = group.at(i)->attributes;
            const auto at =
                std::find_if(attributes.begin(), attributes.end(),
                             [&](const Attribute& current) { return current.key == key; });
            if (at != attributes.end())
                at->data = value;
            else
                attributes.push_back({key, value});
        }
    }

    void write_attributes(Item& item) {
        for (const auto& attribute : item.attributes)
            writer_.attribute(*item.record, attribute.key, attribute.data);
        item.attributes.clear();
    }

    void end(Item& item, Status status, Timestamp time) {
        write_attributes(item);
        writer_.end_at(*item.record, time, status);
        while (!item.names.empty())
            unname(item, item.names.back().first);
        erase_ordered(open_, item);
        release(item);
    }

    std::size_t end_doomed(Status status, Timestamp time) {
        const auto count = doomed_.size();
        for (Item* item : doomed_)
            end(*item, status, time);
        doomed_.clear();
        return count;
    }

    void diag(std::string reason, std::uint32_t space, std::uint64_t key) {
        auto& diagnostic = diagnostics_[std::move(reason)];
        if (!diagnostic.count++ && space < spaces_.size())
            diagnostic.first = spaces_[space]->name + " " + std::to_string(key);
    }

    Writer& writer_;
    Source<Unit, SourceKind::transaction> source_;
    std::string name_;
    StringId label_, empty_;
    std::uint64_t sequence_ = 0;
    std::vector<Item*> open_;
    std::vector<std::unique_ptr<Space>> spaces_;
    std::vector<std::unique_ptr<Item>> pool_;
    std::vector<Item*> free_, moved_, doomed_;
    std::vector<Intern> strings_{512};
    std::map<std::string, Diagnostic> diagnostics_;
};

} // namespace vtr
