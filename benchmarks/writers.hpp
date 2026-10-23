#pragma once

#include "workload.hpp"
#include <vtr_reader.h>
#include <vtr_writer.h>
#include <fstapi.h>
#include <ftr/ftr_writer.h>
#include <charconv>
#include <cstdio>
#include <memory>
#include <unordered_map>

namespace bench {
template<class T> T* require(T* pointer, const char* what) {
    if (!pointer) throw std::runtime_error(what);
    return pointer;
}

struct Writer {
    virtual ~Writer() = default;
    virtual void time(u64) = 0;
    virtual void signal(u64, unsigned, u64) = 0;
    virtual void begin(const Transaction&) = 0;
    virtual void end(const Transaction&) = 0;
    virtual void close() = 0;
};

struct NullWriter : Writer {
    u64 count = 0;
    void time(u64) override {}
    void signal(u64, unsigned, u64) override { ++count; }
    void begin(const Transaction&) override {}
    void end(const Transaction&) override { ++count; }
    void close() override {}
};

class VtrWriter final : public Writer {
    vtr::Writer writer_;
    std::vector<vtr::SignalId> signals_;
    std::vector<vtr::Source<std::array<std::uint64_t, 5>, vtr::SourceKind::transaction>> generators_;
    std::unordered_map<u64, vtr::Transaction<std::array<std::uint64_t, 5>>> active_;

    static vtr::WriterOptions options() {
        vtr::WriterOptions options;
        options.meta.timescale = -9;
        options.mode = vtr::ExecutionMode::inline_;
        return options;
    }

  public:
    VtrWriter(const Profile& profile, const std::string& path) : writer_(path, options()) {
        auto top = writer_.scope("top");
        auto scope = top;
        for (unsigned i = 0; i < profile.signals; ++i) {
            if (i % 32 == 0) {
                scope = writer_.scope("unit_" + std::to_string(i / 32), top);
            }
            signals_.push_back(writer_.signal(
                {.name = "signal_" + std::to_string(i), .parent = scope}, {.width = width(i)}));
        }
        for (unsigned i = 0; i < profile.generators; ++i) {
            generators_.push_back(writer_.transactions<std::array<std::uint64_t, 5>>(
                {.name = "generator_" + std::to_string(i), .parent = top}, "request"));
        }
    }

    void time(u64 time) override {
        writer_.advance_time(time);
    }

    void signal(u64, unsigned signal, u64 value) override {
        writer_.emit(signals_[signal], value);
    }

    void begin(const Transaction& transaction) override {
        auto record = writer_.begin_at(generators_[transaction.generator], transaction.begin, transaction.attrs);
        active_.emplace(transaction.serial, record);
    }

    void end(const Transaction& transaction) override {
        auto record = active_.at(transaction.serial);
        writer_.end_at(record, transaction.end);
        active_.erase(transaction.serial);
    }

    void close() override {
        writer_.close();
    }
};

class FstWriter final : public Writer {
    fstWriterContext* writer_;
    std::vector<fstHandle> handles_;
public:
    FstWriter(const Profile& p, const std::string& path) {
        writer_ = require(fstWriterCreate(path.c_str(), 1), "cannot create FST");
        fstWriterSetPackType(writer_, FST_WR_PT_LZ4);
        fstWriterSetParallelMode(writer_, 0);
        fstWriterSetTimescale(writer_, -9);
        fstWriterSetScope(writer_, FST_ST_VCD_MODULE, "top", nullptr);
        for (unsigned i = 0; i < p.signals; ++i) {
            if (i % 32 == 0) {
                if (i) fstWriterSetUpscope(writer_);
                auto name = "unit_" + std::to_string(i / 32);
                fstWriterSetScope(writer_, FST_ST_VCD_MODULE, name.c_str(), nullptr);
            }
            auto name = "signal_" + std::to_string(i);
            auto id = fstWriterCreateVar(writer_, FST_VT_VCD_WIRE, FST_VD_IMPLICIT, width(i), name.c_str(), 0);
            if (!id) throw std::runtime_error("cannot declare FST signal");
            handles_.push_back(id);
        }
        if (p.signals) fstWriterSetUpscope(writer_);
        fstWriterSetUpscope(writer_);
    }
    ~FstWriter() override { if (writer_) fstWriterClose(writer_); }
    void time(u64 t) override { fstWriterEmitTimeChange(writer_, t); }
    void signal(u64, unsigned s, u64 value) override { fstWriterEmitValueChange64(writer_, handles_[s], width(s), value); }
    void begin(const Transaction&) override {}
    void end(const Transaction&) override {}
    void close() override {
        if (fstWriterGetFseekFailed(writer_)) throw std::runtime_error("FST write failed");
        fstWriterClose(writer_);
        writer_ = nullptr;
    }
};

inline std::string vcd_id(unsigned i) {
    std::string out;
    do { out.push_back(char('!' + i % 94)); i /= 94; } while (i);
    return out;
}

class VcdWriter final : public Writer {
    FILE* file_;
    std::vector<char> buffer_ = std::vector<char>(1024 * 1024);
    std::vector<std::string> ids_;
public:
    VcdWriter(const Profile& p, const std::string& path) {
        file_ = require(std::fopen(path.c_str(), "wb"), "cannot create VCD");
        std::setvbuf(file_, buffer_.data(), _IOFBF, buffer_.size());
        std::fputs("$timescale 1ns $end\n$scope module top $end\n", file_);
        for (unsigned i = 0; i < p.signals; ++i) {
            if (i % 32 == 0) {
                if (i) std::fputs("$upscope $end\n", file_);
                std::fprintf(file_, "$scope module unit_%u $end\n", i / 32);
            }
            ids_.push_back(vcd_id(i));
            std::fprintf(file_, "$var wire %u %s signal_%u $end\n", width(i), ids_.back().c_str(), i);
        }
        if (p.signals) std::fputs("$upscope $end\n", file_);
        std::fputs("$upscope $end\n$enddefinitions $end\n", file_);
    }
    ~VcdWriter() override { if (file_) std::fclose(file_); }
    void time(u64 t) override {
        char line[32];
        line[0] = '#';
        auto end = std::to_chars(line + 1, line + 30, t).ptr;
        *end++ = '\n';
        std::fwrite(line, 1, end - line, file_);
    }
    void signal(u64, unsigned s, u64 value) override {
        char line[96];
        auto end = line;
        if (width(s) == 1) *end++ = char('0' + value);
        else {
            *end++ = 'b';
            bool started = false;
            for (unsigned bit = width(s); bit-- > 0;) {
                const bool one = (value >> bit) & 1;
                if (one || started || bit == 0) { *end++ = one ? '1' : '0'; started = true; }
            }
            *end++ = ' ';
        }
        for (auto c : ids_[s]) *end++ = c;
        *end++ = '\n';
        std::fwrite(line, 1, end - line, file_);
    }
    void begin(const Transaction&) override {}
    void end(const Transaction&) override {}
    void close() override {
        const bool failed = std::ferror(file_);
        auto result = std::fclose(file_);
        file_ = nullptr;
        if (failed || result) throw std::runtime_error("VCD write failed");
    }
};

class FtrWriter final : public Writer {
    std::unique_ptr<ftr::ftr_writer<true>> writer_;
public:
    FtrWriter(const Profile& p, const std::string& path)
        : writer_(std::make_unique<ftr::ftr_writer<true>>(path)) {
        if (!writer_->cw.enc.ofs) throw std::runtime_error("cannot create FTR");
        writer_->writeInfo(-9);
        for (unsigned g = 0; g < p.generators; ++g) {
            if (g % 4 == 0) writer_->writeStream(g / 4, "unit_" + std::to_string(g / 4), "request");
            writer_->writeGenerator(g, "generator_" + std::to_string(g), g / 4);
        }
    }
    void time(u64) override {}
    void signal(u64, unsigned, u64) override {}
    void begin(const Transaction& t) override {
        writer_->startTransaction(t.serial + 1, t.generator, t.generator / 4, t.begin);
        for (unsigned i = 0; i < 4; ++i)
            writer_->writeAttribute(t.serial + 1, ftr::event_type::BEGIN, attr_names[i], ftr::data_type::UNSIGNED, t.attrs[i]);
    }
    void end(const Transaction& t) override {
        writer_->writeAttribute(t.serial + 1, ftr::event_type::END, attr_names[4], ftr::data_type::UNSIGNED, t.attrs[4]);
        writer_->endTransaction(t.serial + 1, t.end);
    }
    void close() override {
        // Flush explicitly to observe I/O failures before the upstream destructor closes the stream.
        writer_->dict.flush(writer_->cw);
        writer_->dir.flush(writer_->cw);
        for (auto& block : writer_->fiber_blocks) if (block) block->flush(writer_->cw);
        writer_->cw.enc.write_break();
        writer_->cw.enc.ofs.close();
        if (!writer_->cw.enc.ofs) throw std::runtime_error("FTR write failed");
        writer_.reset();
    }
};

class PairedWriter final : public Writer {
    std::unique_ptr<Writer> wave_, tx_;
public:
    PairedWriter(std::unique_ptr<Writer> wave, std::unique_ptr<Writer> tx)
        : wave_(std::move(wave)), tx_(std::move(tx)) {}
    void time(u64 t) override { wave_->time(t); }
    void signal(u64 t, unsigned s, u64 v) override { wave_->signal(t, s, v); }
    void begin(const Transaction& t) override { tx_->begin(t); }
    void end(const Transaction& t) override { tx_->end(t); }
    void close() override { wave_->close(); tx_->close(); }
};

inline std::unique_ptr<Writer> writer(const Profile& p, const std::string& format, const std::string& path) {
    if (format == "null") return std::make_unique<NullWriter>();
    if (format == "vtr") return std::make_unique<VtrWriter>(p, path);
    if (format == "fst") return std::make_unique<FstWriter>(p, path);
    if (format == "vcd") return std::make_unique<VcdWriter>(p, path);
    if (format == "ftr") return std::make_unique<FtrWriter>(p, path);
    if (format == "fst+ftr" || format == "vcd+ftr")
        return std::make_unique<PairedWriter>(writer(p, format.substr(0, 3), path + ".wave"), writer(p, "ftr", path + ".tx"));
    throw std::runtime_error("unknown writer: " + format);
}
} // namespace bench
