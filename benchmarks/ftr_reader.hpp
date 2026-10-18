#pragma once

#include "workload.hpp"
#include <fcntl.h>
#include <lz4.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#include <cstring>
#include <map>

namespace bench {
// A reader for the unsigned-attribute FTR subset emitted by these workloads.
class Cbor {
    const unsigned char* pos_;
    const unsigned char* end_;
public:
    Cbor(const unsigned char* data, std::size_t size) : pos_(data), end_(data + size) {}
    unsigned byte() {
        if (pos_ == end_) throw std::runtime_error("truncated FTR CBOR");
        return *pos_++;
    }
    bool done() {
        if (pos_ == end_) throw std::runtime_error("missing CBOR break");
        if (*pos_ != 255) return false;
        ++pos_;
        return true;
    }
    bool exhausted() const { return pos_ == end_; }
    u64 head(unsigned major) {
        auto b = byte();
        if (b >> 5 != major) throw std::runtime_error("unexpected FTR CBOR type");
        unsigned n = b & 31;
        if (n < 24) return n;
        if (n == 31) return unlimited;
        if (n > 27) throw std::runtime_error("invalid FTR CBOR header");
        u64 v = 0;
        for (unsigned i = 0; i < (1u << (n - 24)); ++i) v = (v << 8) | byte();
        return v;
    }
    void expect(unsigned major, u64 value) {
        if (head(major) != value) throw std::runtime_error("unexpected FTR CBOR value");
    }
    std::pair<const unsigned char*, std::size_t> bytes(unsigned major = 2) {
        auto n = head(major);
        if (n > u64(end_ - pos_)) throw std::runtime_error("truncated FTR byte string");
        auto p = pos_;
        pos_ += n;
        return {p, std::size_t(n)};
    }
};

class FtrReader {
    struct Block {
        u64 stream, begin, end, raw_size;
        const unsigned char* data;
        std::size_t size;
        bool compressed;
    };
    const unsigned char* mapping_ = nullptr;
    std::size_t size_ = 0;
    std::vector<Block> blocks_;
    std::map<u64, std::string> dictionary_;
    std::map<u64, unsigned> generators_;
    std::map<unsigned, u64> streams_;

    static std::vector<unsigned char> unpack(const Block& b) {
        if (b.raw_size > 64 * 1024 * 1024 || b.size > 64 * 1024 * 1024)
            throw std::runtime_error("oversized FTR chunk");
        std::vector<unsigned char> out(b.raw_size);
        if (b.compressed) {
            auto n = LZ4_decompress_safe(reinterpret_cast<const char*>(b.data),
                reinterpret_cast<char*>(out.data()), int(b.size), int(out.size()));
            if (n != int(out.size())) throw std::runtime_error("corrupt FTR LZ4 chunk");
        } else std::memcpy(out.data(), b.data, b.size);
        return out;
    }

    void index() {
        Cbor c(mapping_, size_);
        c.expect(6, 55799);
        c.expect(4, unlimited);
        while (!c.done()) {
            const auto tag = c.head(6);
            if (tag == 6) { c.bytes(); continue; }
            if (tag < 8 || tag > 13) throw std::runtime_error("unsupported FTR chunk");
            bool compressed = tag & 1;
            bool tx = tag >= 12;
            if (compressed || tx) c.expect(4, (tx ? 4 : 1) + compressed);
            Block b{};
            b.compressed = compressed;
            if (tx) {
                b.stream = c.head(0);
                b.begin = c.head(0);
                b.end = c.head(0);
            }
            if (compressed) b.raw_size = c.head(0);
            auto [data, size] = c.bytes();
            b.data = data;
            b.size = size;
            if (!compressed) b.raw_size = size;
            if (tx) { blocks_.push_back(b); continue; }
            auto raw = unpack(b);
            Cbor meta(raw.data(), raw.size());
            if (tag <= 9) {
                const auto entries = meta.head(5);
                for (u64 i = 0; entries == unlimited ? !meta.done() : i < entries; ++i) {
                    auto key = meta.head(0);
                    auto [str, len] = meta.bytes(3);
                    dictionary_[key] = std::string(reinterpret_cast<const char*>(str), len);
                }
            } else {
                meta.expect(4, unlimited);
                while (!meta.done()) {
                    auto kind = meta.head(6);
                    meta.expect(4, 3);
                    auto id = meta.head(0), name = meta.head(0), stream = meta.head(0);
                    if (kind == 17) {
                        const auto& label = dictionary_.at(name);
                        if (label.rfind("generator_", 0) != 0) throw std::runtime_error("unexpected FTR generator");
                        auto generator = unsigned(std::stoul(label.substr(10)));
                        generators_[id] = generator;
                        streams_[generator] = stream;
                    } else if (kind != 16) throw std::runtime_error("unexpected FTR directory entry");
                }
            }
            if (!meta.exhausted()) throw std::runtime_error("trailing FTR metadata");
        }
        if (!c.exhausted()) throw std::runtime_error("trailing FTR data");
    }
public:
    explicit FtrReader(const std::string& path) {
        const int fd = open(path.c_str(), O_RDONLY);
        if (fd < 0) throw std::runtime_error("cannot open FTR: " + path);
        struct stat st{};
        if (fstat(fd, &st) || st.st_size == 0) { close(fd); throw std::runtime_error("empty FTR"); }
        size_ = st.st_size;
        auto data = mmap(nullptr, size_, PROT_READ, MAP_PRIVATE, fd, 0);
        close(fd);
        if (data == MAP_FAILED) throw std::runtime_error("cannot map FTR");
        mapping_ = static_cast<const unsigned char*>(data);
        try { index(); }
        catch (...) { munmap(const_cast<unsigned char*>(mapping_), size_); throw; }
    }
    FtrReader(const FtrReader&) = delete;
    FtrReader& operator=(const FtrReader&) = delete;
    ~FtrReader() { munmap(const_cast<unsigned char*>(mapping_), size_); }

    template<class Visitor> void visit(const Query* q, Visitor&& visitor) const {
        for (const auto& b : blocks_) {
            if (q && (b.stream != streams_.at(q->source) || b.begin > q->end || b.end < q->begin)) continue;
            auto raw = unpack(b);
            Cbor c(raw.data(), raw.size());
            c.expect(4, unlimited);
            while (!c.done()) {
                const auto n = c.head(4);
                if (n != 6) throw std::runtime_error("unexpected FTR attribute count");
                c.expect(6, 6);
                c.expect(4, 4);
                Transaction t{};
                t.serial = c.head(0);
                t.generator = generators_.at(c.head(0));
                t.begin = c.head(0);
                t.end = c.head(0);
                unsigned seen = 0;
                for (u64 i = 1; i < n; ++i) {
                    auto tag = c.head(6);
                    if (tag < 7 || tag > 9) throw std::runtime_error("unexpected FTR attribute tag");
                    c.expect(4, 3);
                    const auto& name = dictionary_.at(c.head(0));
                    c.expect(0, 3);
                    auto value = c.head(0);
                    auto it = std::find(attr_names.begin(), attr_names.end(), name);
                    if (it == attr_names.end()) throw std::runtime_error("unknown FTR attribute");
                    auto slot = unsigned(it - attr_names.begin());
                    if (seen & (1u << slot)) throw std::runtime_error("duplicate FTR attribute");
                    seen |= 1u << slot;
                    t.attrs[slot] = value;
                }
                if (seen != 31 || t.serial != t.attrs[0] + 1) throw std::runtime_error("incomplete FTR transaction");
                t.serial = t.attrs[0];
                if (!q || (t.generator == q->source && t.begin <= q->end && t.end >= q->begin && t.attrs[1] == q->value))
                    visitor(t);
            }
            if (!c.exhausted()) throw std::runtime_error("trailing FTR transaction data");
        }
    }
};
} // namespace bench
