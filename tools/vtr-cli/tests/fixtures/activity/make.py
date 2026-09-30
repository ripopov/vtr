#!/usr/bin/env python3
"""Regenerates the FST fixtures of tests/fst_activity.rs with GTKWave's vcd2fst:
one VCD (repeated values, x/z, a real, an alias, two identical signals, long
quiet spans) packed four ways: lz4.fst (-4), fastlz.fst (-F), zlib.fst (-Z)
and wrapped.fst (-Z -c, a gzip-wrapped file). Run from this directory."""
import random
import subprocess

random.seed(7)
L = ["$date 2026-10-01 $end\n$version activity fixture $end\n$timescale 1ps $end", "$scope module top $end"]
L += ["$var wire 1 ! clk $end", "$var wire 8 \" bus [7:0] $end", "$var real 64 # level $end", "$var wire 1 $ copy_a $end",
      "$var wire 1 % copy_b $end", "$var wire 4 & nibble [3:0] $end", "$scope module unit $end", "$var wire 1 ! clk_port $end",
      "$var wire 16 ' data [15:0] $end", "$var wire 1 ( rare $end", "$upscope $end\n$upscope $end\n$enddefinitions $end",
      "#0\n$dumpvars\n0!\nb0 \"\nr0.5 #\n0$\n0%\nbxxxx &\nb0 '\n0(\n$end"]
t, clk, bus, data = 0, 0, 0, 0
for step in range(1, 3000):
    t += random.choice([5, 5, 5, 5, 7, 50, 5000]) if step % 400 else 200000
    out = []
    clk ^= 1
    out.append(f"{clk}!")
    if random.random() < 0.3:
        bus = random.randrange(256)
        out.append(f"b{bus:b} \"")
    elif random.random() < 0.05:
        out.append(f"b{bus:b} \"")  # the same value again
    if random.random() < 0.1:
        out.append(f"r{random.random():.6f} #")
    if random.random() < 0.2:
        v = random.randrange(2)
        out += [f"{v}$", f"{v}%"]
    if random.random() < 0.1:
        out.append("b" + "".join(random.choice("01xz") for _ in range(4)) + " &")
    if random.random() < 0.25:
        data = random.randrange(65536)
        out.append(f"b{data:b} '")
    if step % 997 == 0:
        out.append(f"{random.randrange(2)}(")
    L.append(f"#{t}\n" + "\n".join(out))
open("activity.vcd", "w").write("\n".join(L) + "\n")
for name, flags in [("lz4", ["-4"]), ("fastlz", ["-F"]), ("zlib", ["-Z"]), ("wrapped", ["-Z", "-c"])]:
    subprocess.run(["vcd2fst", *flags, "activity.vcd", f"{name}.fst"], check=True, stdout=subprocess.DEVNULL)
