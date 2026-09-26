// The disassembly hook of the C910 pipeline tracer (c910_tracer.sv): captions
// such as "0x2c04 lh a6,0(t1)" for a PC, from the program's objdump listing
// (+vtr_dis=<file>, default coremark.dis in the working directory). Each
// caption is built once per static instruction and kept, so the returned
// pointer stays valid and the recording interns one string per PC.

#include "verilated.h"

#include <cstdio>
#include <fstream>
#include <regex>
#include <string>
#include <unordered_map>

namespace {

std::unordered_map<uint32_t, std::string>& captions() {
    static std::unordered_map<uint32_t, std::string> table = [] {
        std::unordered_map<uint32_t, std::string> t;
        const std::string arg = Verilated::threadContextp()->commandArgsPlusMatch("vtr_dis=");
        const std::string path = arg.empty() ? "coremark.dis" : arg.substr(std::string{"+vtr_dis="}.size());
        std::ifstream in{path};
        if (!in) VL_PRINTF("%%Warning: c910_vtr_label: cannot read %s; captions are raw PCs\n", path.c_str());
        // "     2c04:\t00031803          \tlh\ta6,0(t1)  # comment"
        static const std::regex line{R"(^\s*([0-9a-f]+):\s+[0-9a-f]+\s+(.*)$)"};
        static const std::regex space{R"(\s+)"};
        std::string s;
        std::smatch m;
        while (std::getline(in, s)) {
            if (!std::regex_match(s, m, line)) continue;
            std::string text = m[2].str();
            text = text.substr(0, text.find('#'));
            text = std::regex_replace(text, space, " ");
            while (!text.empty() && text.back() == ' ') text.pop_back();
            char pc[24];
            std::snprintf(pc, sizeof pc, "0x%04x ", static_cast<unsigned>(std::stoul(m[1].str(), nullptr, 16)));
            t.emplace(static_cast<uint32_t>(std::stoul(m[1].str(), nullptr, 16)), pc + text);
        }
        return t;
    }();
    return table;
}

}  // namespace

extern "C" const char* c910_vtr_label(unsigned int pc) {
    auto& table = captions();
    auto it = table.find(pc);
    if (it == table.end()) {
        char text[24];
        std::snprintf(text, sizeof text, "0x%04x ?", pc);
        it = table.emplace(pc, text).first;
    }
    return it->second.c_str();
}
