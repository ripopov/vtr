#pragma once

#include "wire.h"
#include <algorithm>
#include <limits>

namespace vtr::detail::engine {

struct NumericAttribute {
    wire::Id key;
    wire::Tag tag;
    std::uint64_t number;
};

inline bool numeric_attribute(wire::Tag tag) noexcept {
    return tag <= wire::Tag::f64 || tag == wire::Tag::time || tag == wire::Tag::pointer;
}

struct NumericAttributesView {
    std::span<const NumericAttribute> values;
    std::span<const std::size_t> offsets;

    void validate(std::size_t rows) const {
        if (offsets.empty()) {
            if (!values.empty())
                throw wire::Error(ErrorCode::corrupt_data,
                                  "numeric attributes have no row offsets");
            return;
        }
        if (rows == SIZE_MAX || offsets.size() != rows + 1 || offsets.front() ||
            offsets.back() != values.size() || !std::is_sorted(offsets.begin(), offsets.end()))
            throw wire::Error(ErrorCode::corrupt_data, "invalid numeric attribute row offsets");
    }

    std::span<const NumericAttribute> row(std::size_t index) const {
        if (offsets.empty())
            return {};
        if (index >= offsets.size() - 1 || offsets[index] > offsets[index + 1] ||
            offsets[index + 1] > values.size())
            throw wire::Error(ErrorCode::corrupt_data,
                              "numeric attribute row is outside its storage");
        return values.subspan(offsets[index], offsets[index + 1] - offsets[index]);
    }
};

class NumericAttributeColumns {
    std::vector<NumericAttribute> values_;
    std::vector<std::size_t> offsets_;
    std::size_t rows_ = 0;

  public:
    void append(std::span<const NumericAttribute> values) {
        if (!values.empty()) {
            if (offsets_.empty())
                offsets_.resize(rows_ + 1, 0);
            values_.insert(values_.end(), values.begin(), values.end());
        }
        if (!offsets_.empty())
            offsets_.push_back(values_.size());
        ++rows_;
    }

    NumericAttributesView view() const noexcept {
        return {values_, offsets_};
    }

    void clear() noexcept {
        values_.clear();
        offsets_.clear();
        rows_ = 0;
    }

    std::size_t owned_bytes() const noexcept {
        const auto values = values_.capacity() * sizeof(NumericAttribute);
        const auto offsets = offsets_.capacity() * sizeof(std::size_t);
        return offsets > SIZE_MAX - values ? SIZE_MAX : values + offsets;
    }
};
} // namespace vtr::detail::engine
