#include "payload.h"
#include "scalar_page.h"
#include <iostream>
using namespace vtr::detail::engine;
namespace d = vtr::detail::wire::detail;

void check(bool ok) {
    if (!ok) {
        throw std::runtime_error("scalar page invariant failed");
    }
}

template <class F> void rejects(F&& f) {
    bool failed = false;
    try {
        f();
    } catch (const vtr::detail::wire::Error&) {
        failed = true;
    }
    check(failed);
}

int main(int argc, char** argv) {
    try {
        Types types;
        auto type = types.array(9, 3);
        Generators generators;
        generators.declare({4, RecordKind::ordinary, type});
        generators.declare({9, RecordKind::ordinary, type});
        std::vector<Transaction> rows;
        for (std::uint64_t i = 0; i < 2000; ++i) {
            Transaction tx;
            tx.id = i * 2 + 1;
            tx.generator = i % 2 ? 4 : 9;
            tx.begin = (i * 117) % 9000;
            tx.end = tx.begin + i % 99;
            tx.parent = i ? 1 : 0;
            tx.status = i % 5;
            tx.span_kind = i % 6;
            tx.payload = capture(std::array<std::uint64_t, 3>{i, i % 19, i % 3 ? 0 : UINT64_MAX});
            rows.push_back(std::move(tx));
        }
        for (auto codec : {vtr::detail::wire::Codec::stored, vtr::detail::wire::Codec::lz4,
                           vtr::detail::wire::Codec::zstd}) {
            auto block = encode_transaction_block(7, types, generators, rows, {}, {4096, codec});
            TransactionBlockView view(block, types, generators);
            view.validate();
            unsigned n = 0;
            view.transactions({}, [&](const auto& tx) {
                const auto& expected = rows.at(n++);
                check(tx.id == expected.id && tx.payload == expected.payload &&
                      tx.parent == expected.parent && tx.end == expected.end &&
                      tx.status == expected.status && tx.span_kind == expected.span_kind);
                return true;
            });
            check(n == rows.size());
            if (argc == 2 && unsigned(codec) == std::stoul(argv[1])) {
                for (auto b : block) {
                    std::cout << "0123456789abcdef"[b >> 4] << "0123456789abcdef"[b & 15];
                }
                std::cout << '\n';
                return 0;
            }
        }
        std::vector<const Transaction*> pointers{&rows[0], &rows[1]};
        auto raw =
            scalar_page::encode(types, type, pointers, [](auto g) { return g == 4 ? 0 : 1; });
        const std::array<GeneratorId, 2> ids{4, 9};
        auto decode = [&](View bytes, std::size_t count = 2, std::size_t limit = 64 << 20) {
            return scalar_page::decode(bytes, types, generators, ids, count, limit,
                                       [](const auto&) { return true; });
        };
        check(decode(raw));
        for (std::size_t n = 0; n < raw.size(); ++n) {
            rejects([&] { decode(View(raw).first(n)); });
        }
        auto bad = raw;
        bad[9] = 9;
        rejects([&] { decode(bad); });
        rejects([&] { decode(raw, 3); });
        rejects([&] { decode(raw, 2, 8); });
        bad = raw;
        bad.push_back(0);
        rejects([&] { decode(bad); });
        std::cout << "scalar pages preserve boundaries, typed fields and "
                     "malformed-input checks\n";
    } catch (const std::exception& e) {
        std::cerr << e.what() << '\n';
        return 1;
    }
}
