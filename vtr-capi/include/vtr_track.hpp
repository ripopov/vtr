/* vtr_track.hpp - header-only C++17 keyed lifecycle tracker over the VTR C API. */

#ifndef VTR_TRACK_HPP
#define VTR_TRACK_HPP

#include "vtr.h"

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <map>
#include <memory>
#include <string>
#include <utility>
#include <vector>

namespace vtr {

class Tracker {
public:
    Tracker(vtr_writer *w, uint32_t generator, std::string name)
        : w_(w), gen_(generator), name_(std::move(name)), label_(vtr_writer_intern(w, "vtr.label")) {}
    Tracker(const Tracker &) = delete;
    Tracker &operator=(const Tracker &) = delete;

    vtr_writer *writer() const { return w_; }
    const std::string &name() const { return name_; }

    uint32_t keyspace(const char *name) {
        spaces_.emplace_back(new Space{});
        spaces_.back()->name = name ? name : "";
        return static_cast<uint32_t>(spaces_.size() - 1);
    }
    size_t keyspace_count() const { return spaces_.size(); }
    size_t open_count() const { return open_.size(); }


    void open(uint32_t sp, uint64_t k, const char *stage, uint64_t t) {
        if (!valid(sp, "open")) return;
        if (const Group *g = spaces_[sp]->by_key.find(k)) {
            diag("open on a key that still names an item; that item is aborted", sp, k);
            abort_items(members(*g), t);
        }
        uint64_t tx = 0;
        if (!check(vtr_writer_begin_tx(w_, gen_, t, &tx))) return;
        Item *it = alloc();
        it->seq = ++seq_;
        it->tx = tx;
        open_.push_back(it);
        enter(*it, 0, intern(stage), t);
        name(*it, sp, k);
    }

    void bind(uint32_t sp, uint64_t k, uint32_t to_sp, uint64_t to_k, uint64_t t) {
        if (!valid(sp, "bind") || !valid(to_sp, "bind")) return;
        const Group *g = lookup(sp, k, "bind");
        if (!g) return;
        const Items items = members(*g);
        evict(to_sp, to_k, items, t);
        for (size_t i = 0; i < items.n; ++i) name(*items.v[i], to_sp, to_k);
    }

    int bind_oldest(uint32_t sp, int n, uint32_t to_sp, uint64_t to_k, uint64_t t) {
        if (!valid(sp, "bind_oldest") || !valid(to_sp, "bind_oldest")) return 0;
        const std::vector<Item *> &order = spaces_[sp]->by_seq;
        const size_t count = std::min(order.size(), static_cast<size_t>(std::max(n, 0)));
        if (count < static_cast<size_t>(std::max(n, 0))) diag("bind_oldest found fewer items than requested", to_sp, to_k);
        moved_.assign(order.begin(), order.begin() + static_cast<std::ptrdiff_t>(count));
        Items keep;
        for (Item *it : moved_) keep.push(it);
        evict(to_sp, to_k, keep, t);
        for (Item *it : moved_) {
            unname(*it, sp);
            name(*it, to_sp, to_k);
        }
        return static_cast<int>(count);
    }

    void stage(uint32_t sp, uint64_t k, const char *stage, uint64_t t) {
        const Group *g = lookup(sp, k, "stage");
        if (!g) return;
        const uint32_t s = intern(stage);
        const Items items = members(*g);
        for (size_t i = 0; i < items.n; ++i) enter(*items.v[i], 0, s, t);
    }

    void lane(uint32_t sp, uint64_t k, const char *lane, const char *stage, uint64_t t) {
        const Group *g = lookup(sp, k, "lane");
        if (!g) return;
        const uint32_t l = intern(lane), s = intern(stage);
        const Items items = members(*g);
        for (size_t i = 0; i < items.n; ++i) {
            Item &it = *items.v[i];
            if (s) {
                enter(it, l, s, t);
                continue;
            }
            for (auto &cur : it.lanes) {
                if (cur.first == l && cur.second) {
                    check(vtr_writer_tx_stage_end(w_, it.tx, cur.second, l, t));
                    cur.second = 0;
                }
            }
        }
    }


    void event(uint32_t sp, uint64_t k, const char *name, uint64_t t) {
        const Group *g = lookup(sp, k, "event");
        if (!g) return;
        const uint32_t n = intern(name);
        const Items items = members(*g);
        for (size_t i = 0; i < items.n; ++i) check(vtr_writer_tx_event(w_, items.v[i]->tx, t, n, 0, nullptr, nullptr));
    }

    void attr(uint32_t sp, uint64_t k, const char *key, uint64_t v) {
        const Group *g = lookup(sp, k, "attr");
        if (!g) return;
        vtr_value val{};
        val.tag = VTR_VAL_U64;
        val.u = v;
        set_attr(*g, intern(key), val);
    }

    void attr(uint32_t sp, uint64_t k, const char *key, const char *v) {
        const Group *g = lookup(sp, k, "attr");
        if (!g) return;
        set_attr(*g, intern(key), str_value(v));
    }

    void label(uint32_t sp, uint64_t k, const char *text) {
        const Group *g = lookup(sp, k, "label");
        if (!g) return;
        set_attr(*g, label_, str_value(text));
    }


    void close(uint32_t sp, uint64_t k, uint8_t status, uint64_t t) {
        const Group *g = lookup(sp, k, "close");
        if (!g) return;
        const Items items = members(*g);
        for (size_t i = 0; i < items.n; ++i) end(*items.v[i], status, t);
    }

    int abort_younger(uint32_t sp, uint64_t k, uint64_t t) {
        const Group *g = lookup(sp, k, "abort_younger");
        if (!g) return 0;
        uint64_t last = 0;
        const Items items = members(*g);
        for (size_t i = 0; i < items.n; ++i) last = std::max(last, items.v[i]->seq);
        auto from = std::upper_bound(open_.begin(), open_.end(), last, [](uint64_t s, const Item *it) { return s < it->seq; });
        doomed_.assign(from, open_.end());
        return abort_doomed(t);
    }

    int abort_keyspace(uint32_t sp, uint64_t t) {
        if (!valid(sp, "abort_keyspace")) return 0;
        doomed_ = spaces_[sp]->by_seq;
        return abort_doomed(t);
    }

    int abort_all(uint64_t t) {
        doomed_ = open_;
        return abort_doomed(t);
    }


    std::vector<uint64_t> txs(uint32_t sp, uint64_t k, const char *what) {
        std::vector<uint64_t> out;
        if (const Group *g = lookup(sp, k, what)) {
            const Items items = members(*g);
            for (size_t i = 0; i < items.n; ++i) out.push_back(items.v[i]->tx);
        }
        return out;
    }

    void relate(const char *kind, uint32_t sp, uint64_t k, Tracker &b, uint32_t bsp, uint64_t bk) {
        const Group *from = lookup(sp, k, "relate");
        const Group *to = b.lookup(bsp, bk, "relate");
        if (!from || !to) return;
        const uint32_t kid = intern(kind);
        for (size_t i = 0; i < from->size(); ++i)
            for (size_t j = 0; j < to->size(); ++j)
                check(vtr_writer_relate(w_, kid, from->at(i)->tx, to->at(j)->tx, 0, nullptr, nullptr));
    }

    void parent(uint32_t sp, uint64_t k, Tracker &b, uint32_t bsp, uint64_t bk) {
        const Group *child = lookup(sp, k, "parent");
        const Group *par = b.lookup(bsp, bk, "parent");
        if (!child || !par) return;
        for (size_t i = 0; i < child->size(); ++i) check(vtr_writer_set_tx_parent(w_, child->at(i)->tx, par->at(0)->tx));
    }


    void detach() {
        for (Item *it : open_) {
            write_attrs(*it);
            release(it);
        }
        open_.clear();
        for (auto &s : spaces_) {
            s->by_key.clear();
            s->by_seq.clear();
        }
    }

    struct Diagnostic {
        uint64_t count = 0;
        std::string first;
    };
    const std::map<std::string, Diagnostic> &diagnostics() const { return diags_; }
    std::vector<std::string> take_diagnostics() {
        std::vector<std::string> out;
        for (auto &d : diags_) {
            out.push_back(name_ + ": " + d.first + " (" + std::to_string(d.second.count)
                          + (d.second.count == 1 ? " time" : " times")
                          + (d.second.first.empty() ? "" : ", first " + d.second.first) + ")");
        }
        diags_.clear();
        return out;
    }

private:
    struct Item {
        uint64_t seq = 0;
        uint64_t tx = 0;
        std::vector<std::pair<uint32_t, uint64_t>> names;
        std::vector<std::pair<uint32_t, uint32_t>> lanes;
        std::vector<std::pair<uint32_t, vtr_value>> attrs;
    };

    struct Group {
        static constexpr size_t INLINE = 4;
        Item *v[INLINE] = {};
        uint32_t n = 0;
        std::vector<Item *> more;
        size_t size() const { return n + more.size(); }
        Item *at(size_t i) const { return i < INLINE ? v[i] : more[i - INLINE]; }
    };

    struct Items {
        Item *inl[Group::INLINE];
        std::vector<Item *> heap;
        Item **v = inl;
        size_t n = 0;
        Items() = default;
        Items(const Items &o) : heap(o.heap), n(o.n) {
            std::copy(o.inl, o.inl + Group::INLINE, inl);
            v = n > Group::INLINE ? heap.data() : inl;
        }
        Items &operator=(const Items &) = delete;
        void push(Item *it) {
            if (n < Group::INLINE && v == inl) {
                inl[n++] = it;
                return;
            }
            if (v == inl) heap.assign(inl, inl + n);
            heap.push_back(it);
            v = heap.data();
            ++n;
        }
        bool contains(const Item *it) const { return std::find(v, v + n, it) != v + n; }
    };

    static Items members(const Group &g) {
        Items out;
        for (size_t i = 0; i < g.size(); ++i) out.push(g.at(i));
        return out;
    }

    class KeyTable {
    public:
        const Group *find(uint64_t k) const {
            if (slots_.empty()) return nullptr;
            for (size_t i = hash(k) & mask();; i = (i + 1) & mask()) {
                const Slot &s = slots_[i];
                if (!s.used) return nullptr;
                if (s.key == k) return &s.group;
            }
        }
        Group *find(uint64_t k) { return const_cast<Group *>(static_cast<const KeyTable *>(this)->find(k)); }
        Group &insert(uint64_t k) {
            if ((count_ + 1) * 4 > slots_.size() * 3) grow();
            size_t i = hash(k) & mask();
            while (slots_[i].used && slots_[i].key != k) i = (i + 1) & mask();
            Slot &s = slots_[i];
            if (!s.used) {
                s.used = true;
                s.key = k;
                ++count_;
            }
            return s.group;
        }
        void erase(uint64_t k) {
            if (slots_.empty()) return;
            size_t i = hash(k) & mask();
            while (slots_[i].used && slots_[i].key != k) i = (i + 1) & mask();
            if (!slots_[i].used) return;
            --count_;
            for (size_t j = (i + 1) & mask(); slots_[j].used; j = (j + 1) & mask()) {
                const size_t home = hash(slots_[j].key) & mask();
                if (((j - home) & mask()) >= ((j - i) & mask())) {
                    std::swap(slots_[i], slots_[j]);
                    i = j;
                }
            }
            slots_[i].used = false;
            slots_[i].group.n = 0;
            slots_[i].group.more.clear();
        }
        void clear() {
            for (Slot &s : slots_) {
                s.used = false;
                s.group.n = 0;
                s.group.more.clear();
            }
            count_ = 0;
        }

    private:
        struct Slot {
            uint64_t key = 0;
            bool used = false;
            Group group;
        };
        static size_t hash(uint64_t k) {
            k ^= k >> 33;
            k *= 0xff51afd7ed558ccdULL;
            k ^= k >> 33;
            return static_cast<size_t>(k);
        }
        size_t mask() const { return slots_.size() - 1; }
        void grow() {
            std::vector<Slot> old;
            old.swap(slots_);
            slots_.resize(old.empty() ? 64 : old.size() * 2);
            count_ = 0;
            for (Slot &s : old) {
                if (!s.used) continue;
                Group &g = insert(s.key);
                g = std::move(s.group);
            }
        }
        std::vector<Slot> slots_;
        size_t count_ = 0;
    };

    struct Space {
        std::string name;
        KeyTable by_key;
        std::vector<Item *> by_seq;
    };

    struct InternCache {
        static constexpr size_t SIZE = 512;
        struct Entry {
            uint64_t hash = 0;
            uint32_t id = 0;
            std::string text;
        };
        std::vector<Entry> table = std::vector<Entry>(SIZE);
    };

    uint32_t intern(const char *s) {
        if (!s || !*s) return 0;
        uint64_t h = 1469598103934665603ULL;
        size_t len = 0;
        for (const char *p = s; *p; ++p, ++len) h = (h ^ static_cast<unsigned char>(*p)) * 1099511628211ULL;
        InternCache::Entry &e = strings_.table[h & (InternCache::SIZE - 1)];
        if (e.id && e.hash == h && e.text.size() == len && std::memcmp(e.text.data(), s, len) == 0) return e.id;
        e.hash = h;
        e.text.assign(s, len);
        e.id = vtr_writer_intern(w_, s);
        return e.id;
    }

    vtr_value str_value(const char *s) {
        vtr_value v{};
        v.tag = VTR_VAL_STR;
        v.str_id = intern(s);
        return v;
    }

    Item *alloc() {
        if (free_.empty()) {
            pool_.emplace_back(new Item{});
            return pool_.back().get();
        }
        Item *it = free_.back();
        free_.pop_back();
        return it;
    }

    void release(Item *it) {
        it->names.clear();
        it->lanes.clear();
        it->attrs.clear();
        free_.push_back(it);
    }

    bool valid(uint32_t sp, const char *what) {
        if (sp < spaces_.size()) return true;
        diag(std::string(what) + " with a key space of another tracker", UINT32_MAX, 0);
        return false;
    }

    const Group *lookup(uint32_t sp, uint64_t k, const char *what) {
        if (!valid(sp, what)) return nullptr;
        const Group *g = spaces_[sp]->by_key.find(k);
        if (!g) diag(std::string(what) + " on a key that names no item", sp, k);
        return g;
    }

    static void insert_ordered(std::vector<Item *> &v, Item *it) {
        auto at = v.end();
        while (at != v.begin() && (*(at - 1))->seq > it->seq) --at;
        v.insert(at, it);
    }

    static void erase_ordered(std::vector<Item *> &v, Item *it) {
        auto at = std::lower_bound(v.begin(), v.end(), it->seq, [](const Item *a, uint64_t s) { return a->seq < s; });
        if (at != v.end() && *at == it) v.erase(at);
    }

    void name(Item &it, uint32_t sp, uint64_t k) {
        for (auto &n : it.names) {
            if (n.first != sp) continue;
            if (n.second == k) return;
            unname(it, sp);
            break;
        }
        it.names.emplace_back(sp, k);
        Space &s = *spaces_[sp];
        Group &g = s.by_key.insert(k);
        size_t at = g.size();
        while (at > 0 && g.at(at - 1)->seq > it.seq) --at;
        if (g.size() < Group::INLINE) {
            for (size_t i = g.n; i > at; --i) g.v[i] = g.v[i - 1];
            g.v[at] = &it;
            ++g.n;
        } else {
            std::vector<Item *> all;
            for (size_t i = 0; i < g.size(); ++i) all.push_back(g.at(i));
            all.insert(all.begin() + static_cast<std::ptrdiff_t>(at), &it);
            g.more.assign(all.begin() + Group::INLINE, all.end());
            std::copy(all.begin(), all.begin() + Group::INLINE, g.v);
        }
        insert_ordered(s.by_seq, &it);
    }

    void unname(Item &it, uint32_t sp) {
        for (size_t i = 0; i < it.names.size(); ++i) {
            if (it.names[i].first != sp) continue;
            Space &s = *spaces_[sp];
            const uint64_t k = it.names[i].second;
            if (Group *g = s.by_key.find(k)) {
                std::vector<Item *> rest;
                size_t kept = 0;
                Item *keep[Group::INLINE];
                for (size_t j = 0; j < g->size(); ++j) {
                    Item *m = g->at(j);
                    if (m == &it) continue;
                    if (kept < Group::INLINE) keep[kept++] = m;
                    else rest.push_back(m);
                }
                if (kept == 0) {
                    s.by_key.erase(k);
                } else {
                    std::copy(keep, keep + kept, g->v);
                    g->n = static_cast<uint32_t>(kept);
                    g->more = std::move(rest);
                }
            }
            erase_ordered(s.by_seq, &it);
            it.names.erase(it.names.begin() + static_cast<std::ptrdiff_t>(i));
            return;
        }
    }

    void evict(uint32_t sp, uint64_t k, const Items &keep, uint64_t t) {
        const Group *g = spaces_[sp]->by_key.find(k);
        if (!g) return;
        Items stale;
        for (size_t i = 0; i < g->size(); ++i)
            if (!keep.contains(g->at(i))) stale.push(g->at(i));
        if (!stale.n) return;
        diag("bind to a key that still names another item; that item is aborted", sp, k);
        abort_items(stale, t);
    }

    void enter(Item &it, uint32_t lane, uint32_t stage, uint64_t t) {
        for (auto &cur : it.lanes) {
            if (cur.first != lane) continue;
            if (cur.second == stage) return;
            check(vtr_writer_tx_stage_begin(w_, it.tx, stage, lane, t));
            cur.second = stage;
            return;
        }
        check(vtr_writer_tx_stage_begin(w_, it.tx, stage, lane, t));
        it.lanes.emplace_back(lane, stage);
    }

    void set_attr(const Group &g, uint32_t key, const vtr_value &v) {
        for (size_t i = 0; i < g.size(); ++i) {
            Item *it = g.at(i);
            auto a = std::find_if(it->attrs.begin(), it->attrs.end(), [&](const auto &e) { return e.first == key; });
            if (a != it->attrs.end()) a->second = v;
            else it->attrs.emplace_back(key, v);
        }
    }

    void write_attrs(Item &it) {
        for (auto &a : it.attrs) check(vtr_writer_tx_attr(w_, it.tx, a.first, &a.second));
        it.attrs.clear();
    }

    void end(Item &it, uint8_t status, uint64_t t) {
        write_attrs(it);
        check(vtr_writer_end_tx(w_, it.tx, t, status));
        while (!it.names.empty()) unname(it, it.names.back().first);
        erase_ordered(open_, &it);
        release(&it);
    }

    int abort_items(const Items &items, uint64_t t) {
        for (size_t i = 0; i < items.n; ++i) end(*items.v[i], VTR_TX_STATUS_ABORTED, t);
        return static_cast<int>(items.n);
    }

    int abort_doomed(uint64_t t) {
        std::vector<Item *> items;
        items.swap(doomed_);
        for (Item *it : items) end(*it, VTR_TX_STATUS_ABORTED, t);
        const int n = static_cast<int>(items.size());
        items.clear();
        doomed_.swap(items);
        return n;
    }

    bool check(int status) {
        if (status == VTR_OK) return true;
        diag(std::string("writer: ") + vtr_last_error(), UINT32_MAX, 0);
        return false;
    }

    void diag(const std::string &what, uint32_t sp, uint64_t k) {
        Diagnostic &d = diags_[what];
        if (d.count++ == 0 && sp < spaces_.size()) d.first = spaces_[sp]->name + " " + std::to_string(k);
    }

    vtr_writer *w_;
    uint32_t gen_;
    std::string name_;
    uint32_t label_;
    uint64_t seq_ = 0;
    std::vector<Item *> open_;
    std::vector<std::unique_ptr<Space>> spaces_;
    std::vector<std::unique_ptr<Item>> pool_;
    std::vector<Item *> free_;
    std::vector<Item *> moved_, doomed_;
    InternCache strings_;
    std::map<std::string, Diagnostic> diags_;
};

}

#endif
