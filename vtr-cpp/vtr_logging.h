#pragma once

// Logging conventions carried by generic datatype attributes.
#include "vtr_reader.h"
#include "vtr_writer.h"

namespace vtr::logging {

enum class FormatStyle { brace, printf };

struct CallsiteOptions {
    std::string_view format;
    std::uint64_t severity = 2;
    std::optional<std::string_view> file = {};
    std::optional<std::uint64_t> line = {};
    std::optional<std::string_view> function = {};
    FormatStyle style = FormatStyle::brace;
};

inline std::string_view severity_name(std::uint64_t severity) {
    switch (severity) {
    case 0:
        return "trace";
    case 1:
        return "debug";
    case 2:
        return "info";
    case 3:
        return "warn";
    case 4:
        return "error";
    case 5:
        return "fatal";
    default:
        return "other";
    }
}

inline DatatypeOptions datatype_options(Writer& writer, CallsiteOptions options) {
    DatatypeOptions result{.name = "LogMessage"};
    auto add = [&](std::string_view key, Value value) {
        result.attributes.push_back({writer.intern(key), std::move(value)});
    };
    add("kind", Value(std::string("callsite")));
    add("log.format", Value(std::string(options.format)));
    if (options.style == FormatStyle::printf)
        add("log.format_style", Value(std::string("printf")));
    else if (options.style != FormatStyle::brace)
        throw Error(ErrorCode::invalid_argument, "Unknown log format style");
    if (options.severity > static_cast<std::uint64_t>(INT64_MAX))
        throw Error(ErrorCode::invalid_argument, "Log severity exceeds i64");
    add("log.severity", Value(EnumValue{static_cast<std::int64_t>(options.severity),
                                        writer.intern(severity_name(options.severity))}));
    if (options.file)
        add("log.file", Value(std::string(*options.file)));
    if (options.line)
        add("log.line", Value(*options.line));
    if (options.function)
        add("log.func", Value(std::string(*options.function)));
    return result;
}

template <class T>
PayloadType<T> declare(Writer& writer, CallsiteOptions options, const void* descriptor = nullptr) {
    return writer.datatype<T>(datatype_options(writer, options), descriptor);
}

inline TypeId declare_raw(Writer& writer, CallsiteOptions options, TypeId arguments) {
    return writer.declare_datatype(datatype_options(writer, options),
                                   StructDefinition{{{"value", arguments}}});
}

struct LogMetadata {
    TypeId datatype;
    std::string format;
    std::uint64_t severity;
    std::optional<std::string> file = {}, function = {};
    std::optional<std::uint64_t> line = {};
    FormatStyle style = FormatStyle::brace;
};

inline std::string metadata_text(const Reader& reader, const Value& value) {
    if (const auto text = std::get_if<std::string>(&value.data))
        return *text;
    if (const auto text = std::get_if<StringId>(&value.data))
        return std::string(reader.string(*text));
    throw Error(ErrorCode::invalid_argument, "Log metadata requires text");
}

inline std::optional<LogMetadata> metadata(const Reader& reader, TypeId datatype) {
    if (datatype.value >= reader.types().size())
        throw Error(ErrorCode::invalid_argument, "Unknown datatype");
    const auto& attributes = reader.types()[datatype.value].attributes;
    auto find = [&](std::string_view key) -> const Value* {
        for (const auto& attribute : attributes)
            if (reader.string(attribute.key) == key)
                return &attribute.data;
        return nullptr;
    };
    const auto kind = find("kind");
    if (!kind ||
        (!std::holds_alternative<std::string>(kind->data) &&
         !std::holds_alternative<StringId>(kind->data)) ||
        metadata_text(reader, *kind) != "callsite")
        return std::nullopt;
    const auto format = find("log.format");
    const auto severity = find("log.severity");
    if (!format || !severity || !std::holds_alternative<EnumValue>(severity->data))
        throw Error(ErrorCode::invalid_argument, "Log format and enum severity are required");
    const auto code = std::get<EnumValue>(severity->data).value;
    if (code < 0)
        throw Error(ErrorCode::invalid_argument, "Log severity requires a nonnegative enum");
    LogMetadata result{datatype, metadata_text(reader, *format), static_cast<std::uint64_t>(code)};
    if (const auto style = find("log.format_style")) {
        const auto text = metadata_text(reader, *style);
        if (text == "printf")
            result.style = FormatStyle::printf;
        else if (text != "brace")
            throw Error(ErrorCode::invalid_argument, "Unknown log format style");
    }
    if (const auto file = find("log.file"))
        result.file = metadata_text(reader, *file);
    if (const auto function = find("log.func"))
        result.function = metadata_text(reader, *function);
    if (const auto line = find("log.line")) {
        if (!std::holds_alternative<std::uint64_t>(line->data))
            throw Error(ErrorCode::invalid_argument, "Log line requires u64");
        result.line = std::get<std::uint64_t>(line->data);
    }
    return result;
}

inline std::vector<LogMetadata> callsites(const Reader& reader) {
    std::vector<LogMetadata> result;
    for (const auto& type : reader.types())
        if (auto item = metadata(reader, type.id))
            result.push_back(std::move(*item));
    return result;
}

inline std::string format_log(const Reader& reader, const Record& record) {
    if (record.kind() != RecordKind::log)
        throw Error(ErrorCode::invalid_argument, "Log formatting requires a LOG record");
    const auto declaration = metadata(reader, record.datatype());
    if (!declaration)
        throw Error(ErrorCode::invalid_argument, "Datatype has no log metadata");
    const auto arguments = record.payload()["value"];
    return declaration->style == FormatStyle::printf
               ? reader.format_printf_arguments(declaration->format, arguments)
               : reader.format_arguments(declaration->format, arguments);
}

} // namespace vtr::logging
