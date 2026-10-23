#pragma once

#include "vtr_common.h"

struct Instruction {
    std::uint64_t pc;
    std::uint64_t opcode;
};

template <> struct vtr::Schema<Instruction> {
    static constexpr std::string_view name = "Instruction";
    static constexpr auto fields = std::tuple{
        vtr::field("pc", &Instruction::pc),
        vtr::field("opcode", &Instruction::opcode),
    };
};
