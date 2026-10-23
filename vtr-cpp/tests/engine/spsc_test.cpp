#include "spsc.h"

#include <cstdint>
#include <cstring>
#include <iostream>
#include <stdexcept>
#include <thread>

using vtr::detail::engine::detail::SpscBytes;

void require(bool condition) {
    if (!condition)
        throw std::runtime_error("SPSC byte publication or reuse violated ownership");
}

template <class Counter> void exercise(std::size_t capacity) {
    SpscBytes<Counter> queue(capacity);
    require(!queue.peek());
    auto* first = queue.reserve(capacity);
    require(first && !queue.peek());
    std::memset(first, 0x73, capacity);
    queue.commit(capacity);
    require(!queue.reserve(1));
    const auto* data = queue.peek();
    require(data && data[0] == 0x73 && data[capacity - 1] == 0x73);
    queue.consume(capacity);
    require(!queue.peek());
    require(!queue.reserve(capacity + 1));

    constexpr std::uint32_t count = 100'000;
    std::jthread producer([&] {
        for (std::uint32_t sequence = 0; sequence < count; ++sequence) {
            const std::uint32_t bytes = 8 + sequence % (capacity - 7);
            std::uint8_t* output;
            while (!(output = queue.reserve(bytes)))
                std::this_thread::yield();
            std::memcpy(output, &bytes, 4);
            std::memcpy(output + 4, &sequence, 4);
            for (std::uint32_t i = 8; i < bytes; ++i) {
                output[i] = static_cast<std::uint8_t>(sequence * 31 + i);
            }
            queue.commit(bytes);
        }
    });
    for (std::uint32_t sequence = 0; sequence < count; ++sequence) {
        const std::uint8_t* input;
        while (!(input = queue.peek()))
            std::this_thread::yield();
        std::uint32_t bytes, received;
        std::memcpy(&bytes, input, 4);
        std::memcpy(&received, input + 4, 4);
        require(bytes >= 8 && bytes <= capacity && received == sequence);
        for (std::uint32_t i = 8; i < bytes; ++i) {
            require(input[i] == static_cast<std::uint8_t>(sequence * 31 + i));
        }
        queue.consume(bytes);
    }
    producer.join();
    require(!queue.peek());
}

int main() {
    try {
        for (auto capacity : {64u, 128u, 512u}) {
            exercise<std::uint16_t>(capacity);
            exercise<std::uint64_t>(capacity);
        }
        for (auto invalid : {0u, 32u, 63u, 65u, 32768u}) {
            bool rejected = false;
            try {
                SpscBytes<std::uint16_t> queue(invalid);
            } catch (const std::invalid_argument&) {
                rejected = true;
            }
            require(rejected);
        }
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
