// CLP: the text lines of the baseline are parsed into a logtype (the message
// with variables replaced by placeholders), dictionary variables (tokens that
// are not plain numbers, e.g. names and hex values) and encoded variables
// (decimal integers and floats), then serialized as a CLP IR stream (the
// four-byte encoding used by CLP's logging-library integrations) and
// compressed with zstd. CLP's own parser and encoders are compiled from
// ext/clp; the IR framing mirrors ffi/ir_stream/encoding_methods.cpp.
//   log_clp <n> <out.clp.zst>       format + parse + encode + compress
//   log_clp --decode <file>         decompress + decode back to text (counts bytes)
#include "bench_common.hpp"
#include "workload.hpp"

#include <clp/ffi/encoding_methods.hpp>
#include <clp/ffi/ir_stream/protocol_constants.hpp>
#include <clp/ir/parsing.hpp>
#include <clp/ir/types.hpp>

#include <zstd.h>

#include <cstdio>
#include <cstring>
#include <string>
#include <string_view>
#include <vector>

using namespace simlog;
namespace P = clp::ffi::ir_stream::cProtocol::Payload;
using clp::ir::four_byte_encoded_variable_t;

static void put_le(std::vector<int8_t> &b, uint64_t v, int n) {
    for (int i = 0; i < n; ++i) b.push_back(static_cast<int8_t>((v >> (8 * i)) & 0xff));
}

static bool serialize_event(int64_t ts_delta, std::string_view msg, std::string &logtype, std::vector<int8_t> &ir) {
    auto enc_handler = [&ir](four_byte_encoded_variable_t v) {
        ir.push_back(P::VarFourByteEncoding);
        put_le(ir, static_cast<uint32_t>(v), 4);
    };
    auto dict_handler = [&ir](std::string_view m, size_t b, size_t e) {
        size_t len = e - b;
        if (len <= UINT8_MAX) {
            ir.push_back(P::VarStrLenUByte);
            ir.push_back(static_cast<int8_t>(len));
        } else if (len <= UINT16_MAX) {
            ir.push_back(P::VarStrLenUShort);
            put_le(ir, len, 2);
        } else {
            ir.push_back(P::VarStrLenInt);
            put_le(ir, len, 4);
        }
        ir.insert(ir.end(), reinterpret_cast<const int8_t *>(m.data() + b), reinterpret_cast<const int8_t *>(m.data() + e));
        return true;
    };
    if (!clp::ffi::encode_message_generically<four_byte_encoded_variable_t>(msg, logtype, clp::ir::escape_and_append_const_to_logtype, enc_handler, dict_handler)) return false;
    size_t len = logtype.size();
    if (len <= UINT8_MAX) {
        ir.push_back(P::LogtypeStrLenUByte);
        ir.push_back(static_cast<int8_t>(len));
    } else if (len <= UINT16_MAX) {
        ir.push_back(P::LogtypeStrLenUShort);
        put_le(ir, len, 2);
    } else {
        ir.push_back(P::LogtypeStrLenInt);
        put_le(ir, len, 4);
    }
    ir.insert(ir.end(), reinterpret_cast<const int8_t *>(logtype.data()), reinterpret_cast<const int8_t *>(logtype.data() + len));
    if (INT8_MIN <= ts_delta && ts_delta <= INT8_MAX) {
        ir.push_back(P::TimestampDeltaByte);
        ir.push_back(static_cast<int8_t>(ts_delta));
    } else if (INT16_MIN <= ts_delta && ts_delta <= INT16_MAX) {
        ir.push_back(P::TimestampDeltaShort);
        put_le(ir, static_cast<uint16_t>(ts_delta), 2);
    } else if (INT32_MIN <= ts_delta && ts_delta <= INT32_MAX) {
        ir.push_back(P::TimestampDeltaInt);
        put_le(ir, static_cast<uint32_t>(ts_delta), 4);
    } else {
        ir.push_back(P::TimestampDeltaLong);
        put_le(ir, static_cast<uint64_t>(ts_delta), 8);
    }
    return true;
}

struct ZstdOut {
    FILE *f;
    ZSTD_CStream *cs;
    std::vector<char> obuf;
    uint64_t written = 0;
    explicit ZstdOut(FILE *file, int level) : f(file), cs(ZSTD_createCStream()), obuf(ZSTD_CStreamOutSize()) {
        ZSTD_initCStream(cs, level);
    }
    void write(const int8_t *p, size_t n, bool end) {
        ZSTD_inBuffer in{p, n, 0};
        for (;;) {
            ZSTD_outBuffer out{obuf.data(), obuf.size(), 0};
            size_t rem = ZSTD_compressStream2(cs, &out, &in, end ? ZSTD_e_end : ZSTD_e_continue);
            if (out.pos) {
                std::fwrite(obuf.data(), 1, out.pos, f);
                written += out.pos;
            }
            if (end ? rem == 0 : in.pos == in.size) break;
        }
    }
    ~ZstdOut() { ZSTD_freeCStream(cs); }
};

static int encode(uint64_t n, const char *path) {
    // Pass 1: formatting alone, to report what the text step costs.
    static char line[1024];
    Generator gen;
    Msg m{};
    double f0 = benchutil::now();
    size_t sink = 0;
    for (uint64_t i = 0; i < n; ++i) {
        gen.next(m);
        switch (m.kind) {
#define X(id, SEV, pfmt, ffmt, bfmt, ...)                                                                        \
    case id: sink += std::snprintf(line, sizeof line, "%" PRIu64 " %-5s " pfmt, m.t, sev_name(SEV), __VA_ARGS__); break;
            SIMLOG_KINDS(X)
#undef X
        default: break;
        }
    }
    double f1 = benchutil::now();
    // Pass 2: format + parse + encode + compress.
    FILE *f = std::fopen(path, "wb");
    if (!f) return 1;
    ZstdOut z(f, 3);
    std::vector<int8_t> ir;
    ir.reserve(1 << 22);
    // Preamble: magic + JSON metadata (four-byte encoding, reference timestamp 0).
    for (auto b : clp::ffi::ir_stream::cProtocol::FourByteEncodingMagicNumber) ir.push_back(b);
    std::string meta = "{\"VERSION\":\"0.0.2\",\"VARIABLES_SCHEMA_ID\":\"com.yscope.clp.VariablesSchemaV2\","
                       "\"VARIABLE_ENCODING_METHODS_ID\":\"com.yscope.clp.VariableEncodingMethodsV1\","
                       "\"TIMESTAMP_PATTERN\":\"%s\",\"TIMESTAMP_PATTERN_SYNTAX\":\"\",\"TZ_ID\":\"UTC\",\"REFERENCE_TIMESTAMP\":\"0\"}";
    ir.push_back(clp::ffi::ir_stream::cProtocol::Metadata::EncodingJson);
    ir.push_back(clp::ffi::ir_stream::cProtocol::Metadata::LengthUShort);
    put_le(ir, meta.size(), 2);
    ir.insert(ir.end(), reinterpret_cast<const int8_t *>(meta.data()), reinterpret_cast<const int8_t *>(meta.data() + meta.size()));
    Generator gen2;
    std::string logtype;
    int64_t prev_t = 0;
    double t0 = benchutil::now();
    for (uint64_t i = 0; i < n; ++i) {
        gen2.next(m);
        int len = 0;
        switch (m.kind) {
#define X(id, SEV, pfmt, ffmt, bfmt, ...)                                                                        \
    case id: len = std::snprintf(line, sizeof line, "%" PRIu64 " %-5s " pfmt, m.t, sev_name(SEV), __VA_ARGS__); break;
            SIMLOG_KINDS(X)
#undef X
        default: break;
        }
        // The leading integer is the timestamp (as CLP's timestamp patterns extract it).
        char *sp = static_cast<char *>(std::memchr(line, ' ', len));
        int64_t ts = static_cast<int64_t>(std::strtoull(line, nullptr, 10));
        std::string_view msg(sp, line + len - sp);
        serialize_event(ts - prev_t, msg, logtype, ir);
        prev_t = ts;
        if (ir.size() >= (1u << 22)) {
            z.write(ir.data(), ir.size(), false);
            ir.clear();
        }
    }
    ir.push_back(clp::ffi::ir_stream::cProtocol::Eof);
    z.write(ir.data(), ir.size(), true);
    double t1 = benchutil::now();
    std::fclose(f);
    double t2 = benchutil::now();
    benchutil::Result r{"clp-ir"};
    r.n = n;
    r.loop_s = t1 - t0;
    r.total_s = t2 - t0;
    r.format_s = f1 - f0;
    r.cpu_s = benchutil::cpu_seconds();
    r.bytes = benchutil::file_size(path);
    (void)sink;
    benchutil::print_json(r);
    return 0;
}

// ---- decode ----
static std::vector<int8_t> read_zstd(const char *path) {
    FILE *f = std::fopen(path, "rb");
    std::vector<char> in;
    char buf[1 << 16];
    size_t k;
    while ((k = std::fread(buf, 1, sizeof buf, f)) > 0) in.insert(in.end(), buf, buf + k);
    std::fclose(f);
    ZSTD_DStream *ds = ZSTD_createDStream();
    ZSTD_initDStream(ds);
    std::vector<int8_t> out;
    out.reserve(in.size() * 4);
    std::vector<int8_t> ob(ZSTD_DStreamOutSize());
    ZSTD_inBuffer zin{in.data(), in.size(), 0};
    while (zin.pos < zin.size) {
        ZSTD_outBuffer zo{ob.data(), ob.size(), 0};
        size_t rc = ZSTD_decompressStream(ds, &zo, &zin);
        if (ZSTD_isError(rc)) break;
        out.insert(out.end(), ob.begin(), ob.begin() + zo.pos);
    }
    ZSTD_freeDStream(ds);
    return out;
}

static int decode(const char *path, bool print) {
    std::vector<int8_t> ir = read_zstd(path);
    double t0 = benchutil::now();
    size_t p = 4; // magic
    auto u8 = [&](size_t i) { return static_cast<uint8_t>(ir[i]); };
    // metadata
    if (u8(p) == clp::ffi::ir_stream::cProtocol::Metadata::EncodingJson) {
        ++p;
        uint8_t lt = u8(p++);
        size_t len = lt == clp::ffi::ir_stream::cProtocol::Metadata::LengthUByte ? u8(p) : (u8(p) | (u8(p + 1) << 8));
        p += lt == clp::ffi::ir_stream::cProtocol::Metadata::LengthUByte ? 1 : 2;
        p += len;
    }
    uint64_t lines = 0, bytes = 0;
    int64_t ts = 0;
    std::vector<std::string_view> dict;
    std::vector<uint32_t> enc;
    std::string out;
    while (p < ir.size() && ir[p] != clp::ffi::ir_stream::cProtocol::Eof) {
        dict.clear();
        enc.clear();
        std::string_view logtype;
        bool have_lt = false;
        while (!have_lt) {
            uint8_t tag = u8(p++);
            if (tag == static_cast<uint8_t>(P::VarFourByteEncoding)) {
                enc.push_back(u8(p) | (u8(p + 1) << 8) | (u8(p + 2) << 16) | (static_cast<uint32_t>(u8(p + 3)) << 24));
                p += 4;
            } else if (tag == static_cast<uint8_t>(P::VarStrLenUByte) || tag == static_cast<uint8_t>(P::VarStrLenUShort) || tag == static_cast<uint8_t>(P::VarStrLenInt)) {
                size_t len = tag == static_cast<uint8_t>(P::VarStrLenUByte) ? u8(p) : tag == static_cast<uint8_t>(P::VarStrLenUShort) ? (u8(p) | (u8(p + 1) << 8)) : (u8(p) | (u8(p + 1) << 8) | (u8(p + 2) << 16) | (u8(p + 3) << 24));
                p += tag == static_cast<uint8_t>(P::VarStrLenUByte) ? 1 : tag == static_cast<uint8_t>(P::VarStrLenUShort) ? 2 : 4;
                dict.emplace_back(reinterpret_cast<const char *>(&ir[p]), len);
                p += len;
            } else if (tag == static_cast<uint8_t>(P::LogtypeStrLenUByte) || tag == static_cast<uint8_t>(P::LogtypeStrLenUShort) || tag == static_cast<uint8_t>(P::LogtypeStrLenInt)) {
                size_t len = tag == static_cast<uint8_t>(P::LogtypeStrLenUByte) ? u8(p) : tag == static_cast<uint8_t>(P::LogtypeStrLenUShort) ? (u8(p) | (u8(p + 1) << 8)) : (u8(p) | (u8(p + 1) << 8) | (u8(p + 2) << 16) | (u8(p + 3) << 24));
                p += tag == static_cast<uint8_t>(P::LogtypeStrLenUByte) ? 1 : tag == static_cast<uint8_t>(P::LogtypeStrLenUShort) ? 2 : 4;
                logtype = std::string_view(reinterpret_cast<const char *>(&ir[p]), len);
                p += len;
                have_lt = true;
            } else {
                std::fprintf(stderr, "bad IR tag %02x at %zu\n", tag, p - 1);
                return 1;
            }
        }
        uint8_t tt = u8(p++);
        int64_t d;
        if (tt == static_cast<uint8_t>(P::TimestampDeltaByte)) {
            d = static_cast<int8_t>(u8(p));
            p += 1;
        } else if (tt == static_cast<uint8_t>(P::TimestampDeltaShort)) {
            d = static_cast<int16_t>(u8(p) | (u8(p + 1) << 8));
            p += 2;
        } else if (tt == static_cast<uint8_t>(P::TimestampDeltaInt)) {
            d = static_cast<int32_t>(u8(p) | (u8(p + 1) << 8) | (u8(p + 2) << 16) | (static_cast<uint32_t>(u8(p + 3)) << 24));
            p += 4;
        } else {
            uint64_t v = 0;
            for (int i = 0; i < 8; ++i) v |= static_cast<uint64_t>(u8(p + i)) << (8 * i);
            d = static_cast<int64_t>(v);
            p += 8;
        }
        ts += d;
        // Reconstruct the text.
        out.clear();
        out += std::to_string(ts);
        size_t di = 0, ei = 0;
        for (size_t i = 0; i < logtype.size(); ++i) {
            char c = logtype[i];
            if (c == static_cast<char>(clp::ir::VariablePlaceholder::Escape)) {
                out += logtype[++i];
            } else if (c == static_cast<char>(clp::ir::VariablePlaceholder::Dictionary)) {
                out += dict[di++];
            } else if (c == static_cast<char>(clp::ir::VariablePlaceholder::Integer)) {
                out += std::to_string(static_cast<int32_t>(enc[ei++]));
            } else if (c == static_cast<char>(clp::ir::VariablePlaceholder::Float)) {
                out += clp::ffi::decode_float_var<four_byte_encoded_variable_t>(static_cast<four_byte_encoded_variable_t>(enc[ei++]));
            } else {
                out += c;
            }
        }
        ++lines;
        bytes += out.size() + 1;
        if (print) std::printf("%s\n", out.c_str());
    }
    double t1 = benchutil::now();
    benchutil::Result r{"clp-ir"};
    r.n = lines;
    r.lines = lines;
    r.bytes = bytes;
    r.decode_s = t1 - t0;
    r.cpu_s = benchutil::cpu_seconds();
    benchutil::print_json(r, print ? stderr : stdout);
    return 0;
}

int main(int argc, char **argv) {
    if (argc >= 3 && std::strcmp(argv[1], "--decode") == 0) return decode(argv[2], argc > 3 && std::strcmp(argv[3], "--print") == 0);
    if (argc < 3) {
        std::fprintf(stderr, "usage: %s <n> <out.clp.zst> | --decode <file> [--print]\n", argv[0]);
        return 2;
    }
    return encode(benchutil::parse_count(argv[1]), argv[2]);
}
