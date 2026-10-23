#pragma once
#include <ankerl/unordered_dense.h>

namespace vtr::detail::wire::detail {
template <class Key, class Value> using HashMap = ankerl::unordered_dense::map<Key, Value>;
template <class Key> using HashSet = ankerl::unordered_dense::set<Key>;

struct StringHash {
    using is_transparent = void;
    using is_avalanching = void;

    auto operator()(std::string_view text) const noexcept {
        return ankerl::unordered_dense::hash<std::string_view>{}(text);
    }
};
template <class Value>
using StringMap = ankerl::unordered_dense::map<std::string, Value, StringHash, std::equal_to<>>;
} // namespace vtr::detail::wire::detail
