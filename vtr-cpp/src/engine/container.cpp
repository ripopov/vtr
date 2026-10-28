#include "engine/container.h"
#include "format.h"
#include <filesystem>
#include <fstream>
#include <unordered_set>

namespace vtr::detail::engine {
namespace d = vtr::detail::wire::detail;

RecordKind record_kind(std::string_view kind) {
    if (kind == "CLOCK") {
        return RecordKind::clock;
    }
    if (kind == "EVENT") {
        return RecordKind::event;
    }
    if (kind == "STAGE") {
        return RecordKind::stage;
    }
    if (kind == "LOG") {
        return RecordKind::log;
    }
    return RecordKind::ordinary;
}

ContainerReader::ContainerReader(const std::string& path, bool verify_checksums,
                                 std::size_t max_payload_bytes)
    : native_(path, {.verify_crc = verify_checksums}) {
    auto nodes = native_.nodes();
    std::size_t section_index = 0;
    std::uint64_t string_count = 0;
    for (const auto& section : native_.sections()) {
        if (section.kind == 2) {
            auto raw = d::decompress(native_.section_data(section));
            d::Cursor r(raw);
            auto first = r.var(), count = r.var();
            string_count = first + count;
        } else if (section.kind == 10) {
            types_->read_declarations(native_.section_data(section), string_count);
        } else if (section.kind == 3) {
            auto raw = d::decompress(native_.section_data(section));
            d::Cursor r(raw);
            auto first = r.id(), count = r.id();
            d::require(first <= nodes.size() && count <= nodes.size() - first,
                       "hierarchy chunk exceeds nodes");
            for (std::size_t id = first; id < std::size_t(first) + count; ++id) {
                const auto& node = nodes[id];
                if (node.kind != vtr::detail::wire::Node::generator) {
                    continue;
                }
                (*types_)[node.payload_type];
                generators_.declare({static_cast<GeneratorId>(id),
                                     record_kind(native_.string(node.aux)), node.payload_type});
            }
        } else if (section.kind == 5 || section.kind == 9) {
            d::require(blocks_.emplace(section.block_number, section_index).second,
                       "duplicate block number");
            block_strings_[section.block_number] = string_count;
            if (section.kind == 9) {
                auto parsed =
                    std::make_unique<RecordBlockView>(native_.section_data(section), *types_,
                                                      generators_, string_count, max_payload_bytes);
                const auto& view = *parsed;
                d::require(view.number() == section.block_number && view.begin() == section.aux0 &&
                               view.end() == section.aux1,
                           "record block directory mismatch");
                record_views_.emplace(section.block_number, std::move(parsed));
            } else {
                auto parsed = std::make_unique<TransactionBlockView>(
                    native_.section_data(section), *types_, generators_, string_count,
                    max_payload_bytes, native_.strings());
                const auto& view = *parsed;
                d::require(view.number() == section.block_number &&
                               view.bounds().begin == section.aux0 &&
                               view.bounds().end == section.aux1,
                           "transaction block directory mismatch");
                transaction_views_.emplace(section.block_number, std::move(parsed));
            }
        }
        ++section_index;
    }
}

const vtr::detail::wire::ContainerSection* ContainerReader::block(std::uint64_t number) const {
    auto i = blocks_.find(number);
    return i == blocks_.end() ? nullptr : &native_.sections()[i->second];
}

const RecordBlockView* ContainerReader::record_block(std::uint64_t number) const {
    auto i = record_views_.find(number);
    return i == record_views_.end() ? nullptr : i->second.get();
}

const TransactionBlockView* ContainerReader::transaction_block(std::uint64_t number) const {
    auto i = transaction_views_.find(number);
    return i == transaction_views_.end() ? nullptr : i->second.get();
}

void ContainerReader::records(RecordQuery query,
                              const std::function<bool(const LocatedRecord&)>& visit) const {
    bool keep_going = true;
    for (const auto& section : native_.sections()) {
        if (section.kind != 9 || query.end < section.aux0 || query.begin > section.aux1) {
            continue;
        }
        native_.section_data(section);
        const auto& view = *record_block(section.block_number);
        view.query(query, [&](const auto& record) {
            keep_going = visit(record);
            return keep_going;
        });
        if (!keep_going) {
            return;
        }
    }
}

void ContainerReader::transactions(TransactionQuery query,
                                   const std::function<bool(const Transaction&)>& visit) const {
    bool keep_going = true;
    for (const auto& section : native_.sections()) {
        if (section.kind != 5 || query.end < section.aux0 || query.begin > section.aux1) {
            continue;
        }
        native_.section_data(section);
        const auto& view = *transaction_block(section.block_number);
        view.transactions(query, [&](const auto& tx) {
            keep_going = visit(tx);
            return keep_going;
        });
        if (!keep_going) {
            return;
        }
    }
}

void ContainerReader::relations(RecordId endpoint, bool incoming,
                                const std::function<bool(const Relation&)>& visit) const {
    bool keep_going = true;
    for (const auto& section : native_.sections()) {
        if (section.kind != 5) {
            continue;
        }
        native_.section_data(section);
        const auto& view = *transaction_block(section.block_number);
        view.relations(endpoint, incoming, [&](const auto& relation) {
            keep_going = visit(relation);
            return keep_going;
        });
        if (!keep_going) {
            return;
        }
    }
}

void ContainerReader::validate_transaction_blocks() const {
    for (const auto& section : native_.sections()) {
        if (section.kind != 5) {
            continue;
        }
        TransactionBlockView view(native_.section_data(section, true), *types_, generators_,
                                  block_strings_.at(section.block_number), default_payload_limit,
                                  native_.strings());
        view.validate();
    }
}

void ContainerReader::validate_record_blocks() const {
    std::unordered_set<RecordId> ids;
    for (const auto& section : native_.sections()) {
        if (section.kind != 9) {
            continue;
        }
        RecordBlockView view(native_.section_data(section, true), *types_, generators_,
                             block_strings_.at(section.block_number));
        view.validate();
        view.query({}, [&](const auto& record) {
            d::require(ids.insert(record.id).second, "duplicate global record identifier");
            types_->validate(generators_.payload_type(record.generator, record.datatype()),
                             record.payload, native_.strings().size());
            return true;
        });
    }
}

void recover_container(const std::string& input, const std::string& output) {
    std::error_code error;
    d::require(input != output && !std::filesystem::equivalent(input, output, error),
               "recovery paths refer to the same file");
    std::ifstream in(input, std::ios::binary);
    d::require(bool(in), "cannot open recovery input");
    auto size = std::filesystem::file_size(input);
    Bytes file_header(32);
    in.read(reinterpret_cast<char*>(file_header.data()), file_header.size());
    d::require(
        std::size_t(in.gcount()) == file_header.size() &&
            std::equal(d::magic.begin(), d::magic.end(), file_header.begin()) &&
            d::load<std::uint16_t>(View(file_header).subspan(8)) == d::format_major &&
            d::load<std::uint16_t>(View(file_header).subspan(10)) == d::format_minor &&
            std::all_of(file_header.begin() + 12, file_header.begin() + 16,
                        [](auto b) { return b == 0; }) &&
            std::any_of(file_header.begin() + 16, file_header.end(), [](auto b) { return b != 0; }),
        "invalid recovery file header");
    std::vector<vtr::detail::wire::ContainerSection> sections;
    std::array<char, 65536> buffer;
    std::uint64_t prefix = 32;
    while (prefix <= size && size - prefix >= 24) {
        in.clear();
        in.seekg(prefix);
        std::array<std::uint8_t, 24> header;
        in.read(reinterpret_cast<char*>(header.data()), header.size());
        if (std::size_t(in.gcount()) != header.size()) {
            break;
        }
        d::Cursor h(header);
        vtr::detail::wire::ContainerSection section;
        section.kind = h.fixed<std::uint32_t>();
        section.flags = h.fixed<std::uint32_t>();
        section.offset = prefix;
        section.length = h.fixed<std::uint64_t>();
        auto expected_crc = h.fixed<std::uint32_t>();
        if (h.fixed<std::uint32_t>() || !section.kind || section.kind == 7 ||
            section.length > size - prefix - 24) {
            break;
        }
        std::uint32_t crc = 0;
        Bytes leading;
        bool complete = true;
        for (std::uint64_t at = 0; at < section.length;) {
            auto n = std::min<std::uint64_t>(buffer.size(), section.length - at);
            in.read(buffer.data(), n);
            if (std::uint64_t(in.gcount()) != n) {
                complete = false;
                break;
            }
            View bytes(reinterpret_cast<const std::uint8_t*>(buffer.data()), n);
            crc = d::checksum(bytes, crc);
            if (at == 0) {
                leading.assign(bytes.begin(),
                               bytes.begin() + std::min<std::size_t>(64, bytes.size()));
            }
            at += n;
        }
        if (!complete || (expected_crc && crc != expected_crc)) {
            break;
        }
        try {
            d::Cursor bounds(leading);
            if (section.kind == 4) {
                section.aux0 = bounds.fixed<std::uint64_t>();
                section.aux1 = bounds.fixed<std::uint64_t>();
            } else if (section.kind == 5 || section.kind == 9) {
                section.block_number = bounds.fixed<std::uint64_t>();
                bounds.take(section.kind == 5 ? 32 : 24);
                section.aux0 = bounds.fixed<std::uint64_t>();
                section.aux1 = bounds.fixed<std::uint64_t>();
            } else if (section.kind == 10) {
                section.aux0 = bounds.var();
                section.aux1 = bounds.var();
            } else if (section.kind == 2 || section.kind == 3) {
                in.clear();
                in.seekg(prefix + 24);
                Bytes encoded(section.length);
                in.read(reinterpret_cast<char*>(encoded.data()), encoded.size());
                d::require(std::size_t(in.gcount()) == encoded.size(),
                           "truncated recovered declaration");
                auto raw = d::decompress(encoded);
                d::Cursor declarations(raw);
                section.aux0 = declarations.var();
                section.aux1 = declarations.var();
            }
        } catch (const vtr::detail::wire::Error&) {
            break;
        }
        sections.push_back(section);
        prefix += 24 + section.length;
    }
    d::require(std::any_of(sections.begin(), sections.end(),
                           [](const auto& section) { return section.kind == 1; }),
               "recovery prefix has no complete metadata");
    std::ofstream out(output, std::ios::binary | std::ios::trunc);
    d::require(bool(out), "cannot open recovery output");
    in.clear();
    in.seekg(0);
    for (std::uint64_t at = 0; at < prefix;) {
        auto n = std::min<std::uint64_t>(buffer.size(), prefix - at);
        in.read(buffer.data(), n);
        d::require(std::uint64_t(in.gcount()) == n, "cannot read recovery input");
        out.write(buffer.data(), n);
        at += n;
    }
    Bytes directory;
    d::fixed<std::uint64_t>(directory, sections.size());
    d::fixed<std::uint64_t>(directory, size - prefix);
    for (const auto& section : sections) {
        d::fixed(directory, section.kind);
        d::fixed(directory, section.flags);
        d::fixed(directory, section.offset);
        d::fixed(directory, section.length);
        d::fixed(directory, section.aux0);
        d::fixed(directory, section.aux1);
        d::fixed(directory, section.block_number);
    }
    Bytes header;
    d::fixed<std::uint32_t>(header, 7);
    d::fixed<std::uint32_t>(header, 2);
    d::fixed<std::uint64_t>(header, directory.size());
    d::fixed(header, d::checksum(directory));
    d::fixed<std::uint32_t>(header, 0);
    out.write(reinterpret_cast<const char*>(header.data()), header.size());
    out.write(reinterpret_cast<const char*>(directory.data()), directory.size());
    Bytes trailer;
    d::fixed(trailer, prefix);
    d::fixed<std::uint64_t>(trailer, prefix + 24 + directory.size() + 24);
    d::append(trailer, d::trailer);
    out.write(reinterpret_cast<const char*>(trailer.data()), trailer.size());
    out.close();
    d::require(bool(out), "cannot close recovered file");
}
} // namespace vtr::detail::engine
