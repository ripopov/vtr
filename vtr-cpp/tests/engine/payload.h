#pragma once

#include "engine/schema.h"

template <class T> vtr::detail::engine::Bytes capture(const T& value) {
    vtr::detail::engine::Payload payload;
    payload.append(value);
    return std::move(payload).take();
}
