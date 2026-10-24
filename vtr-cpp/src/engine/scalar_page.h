#pragma once
#include "engine/transactions.h"
#include "format.h"

namespace vtr::detail::engine::scalar_page {
namespace d = vtr::detail::wire::detail;

inline bool eligible(const Types& types, const Generators& generators,
                     std::span<const Transaction> rows, TypeId& type) {
    if (rows.empty()) {
        return false;
    }
    type = generators[rows.front().generator].payload_type;
    const auto& schema = types[type];
    if (!schema.unsigned_words || !schema.fixed_size || *schema.fixed_size > 2048) {
        return false;
    }
    for (const auto& tx : rows) {
        if (generators[tx.generator].kind != RecordKind::ordinary ||
            generators[tx.generator].payload_type != type || !tx.attributes.empty() ||
            !tx.events.empty() || !tx.stages.empty() || !tx.child_blocks.empty()) {
            return false;
        }
    }
    return true;
}

inline Bytes encode(const Types& types, TypeId type, std::span<const Transaction* const> rows,
                    const std::function<std::uint32_t(GeneratorId)>& local) {
    const auto payload = *types[type].fixed_size;
    const auto count = rows.size();
    const auto columns = 6 + payload / 8;
    std::vector<std::uint64_t> values(count * columns);
    for (std::size_t i = 0; i < count; ++i) {
        const auto& tx = *rows[i];
        d::require(tx.payload.size() == payload, "scalar payload size mismatch");
        values[i] = tx.id;
        values[count + i] = local(tx.generator);
        values[2 * count + i] = tx.begin;
        values[3 * count + i] = tx.end - tx.begin;
        values[4 * count + i] = tx.parent;
        values[5 * count + i] = (tx.span_kind << 4) | tx.status;
        for (std::size_t n = 0; n < payload / 8; ++n) {
            values[(6 + n) * count + i] =
                d::load<std::uint64_t>(View(tx.payload).subspan(n * 8, 8));
        }
    }
    Bytes out{2};
    d::fixed(out, type);
    d::fixed(out, std::uint32_t(count));
    for (std::size_t c = 0; c < columns; ++c) {
        auto* column = values.data() + c * count;
        std::uint64_t used = 0;
        for (std::size_t i = 0; i < count; ++i) {
            used |= column[i];
        }
        auto width = unsigned((64 - std::countl_zero(used) + 7) / 8);
        out.push_back(width);
        auto at = out.size();
        out.resize(at + width * count);
        for (unsigned byte = 0; byte < width; ++byte) {
            for (std::size_t i = 0; i < count; ++i) {
                out[at + byte * count + i] = column[i] >> (byte * 8);
            }
        }
    }
    return out;
}

template <class Visit>
bool decode(View raw, const Types& types, const Generators& generators,
            std::span<const GeneratorId> ids, std::uint64_t expected, std::size_t limit,
            Visit&& visit) {
    d::Cursor in(raw);
    d::require(in.byte() == 2, "invalid scalar page version");
    auto type = in.fixed<TypeId>(), count = in.fixed<std::uint32_t>();
    const auto& schema = types[type];
    d::require(count && count == expected && schema.unsigned_words && schema.fixed_size &&
                   *schema.fixed_size <= 2048 && *schema.fixed_size <= limit,
               "invalid scalar page schema/count");
    auto payload = *schema.fixed_size, columns = 6 + payload / 8;
    d::require(count <= raw.size(), "invalid scalar page row count");
    std::vector<View> planes;
    std::vector<unsigned> widths;
    for (std::size_t c = 0; c < columns; ++c) {
        unsigned width = in.byte();
        d::require(width <= 8, "invalid scalar column width");
        auto data = in.take(std::size_t(width) * count);
        d::require(!width ||
                       std::any_of(data.end() - count, data.end(), [](auto b) { return b != 0; }),
                   "noncanonical scalar column width");
        planes.push_back(data);
        widths.push_back(width);
    }
    in.done();
    auto value = [&](std::size_t c, std::size_t row) {
        std::uint64_t n = 0;
        for (unsigned byte = 0; byte < widths[c]; ++byte) {
            n |= std::uint64_t(planes[c][byte * count + row]) << (byte * 8);
        }
        return n;
    };
    Transaction tx;
    tx.payload.reserve(payload);
    for (std::size_t i = 0; i < count; ++i) {
        tx.payload.clear();
        tx.id = value(0, i);
        auto gen = value(1, i);
        d::require(gen < ids.size(), "invalid scalar local generator");
        tx.generator = ids[gen];
        const auto& generator = generators[tx.generator];
        d::require(generator.kind == RecordKind::ordinary && generator.payload_type == type,
                   "scalar generator schema mismatch");
        tx.begin = value(2, i);
        auto duration = value(3, i);
        d::require(duration <= UINT64_MAX - tx.begin, "scalar interval overflows");
        tx.end = tx.begin + duration;
        tx.parent = value(4, i);
        auto classification = value(5, i);
        d::require(classification <= 255, "scalar classification overflows");
        tx.status = classification & 15;
        tx.span_kind = classification >> 4;
        for (std::size_t c = 6; c < columns; ++c) {
            d::fixed(tx.payload, value(c, i));
        }
        if (!visit(tx)) {
            return false;
        }
    }
    return true;
}
} // namespace vtr::detail::engine::scalar_page
