#!/usr/bin/env python3
"""SREC to Verilog $readmemh pattern converter (portable replacement for
T-Head's Linux Srec2vmem binary, used by bench/workloads/c910 on hosts where
that static x86-64 executable cannot run).

Reads a Motorola SREC file (S0/S1/S2/S3 data records) and writes a .pat file
that the smart_run testbench loads with $readmemh: an `@addr` line followed by
one 32-bit hex word per line. SREC addresses are byte addresses; each word is
the four bytes at that address packed big-endian (byte at A becomes the MSB),
matching the byte-lane layout the testbench builds in the SoC RAM banks.

Usage: Srec2vmem.py <input.srec> <output.pat>
"""

import sys


def parse_srec(path):
    """Return a byte string covering [min_addr, max_addr+1) with holes as 0."""
    recs = []  # (addr, bytes)
    min_addr, max_addr = None, None
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line or line[0] != "S":
                continue
            kind = line[1]
            if kind in ("0", "5", "7", "8", "9"):
                continue  # header, count, start-address records
            if kind not in ("1", "2", "3"):
                raise ValueError(f"unexpected SREC record type S{kind}")
            addr_len = {"1": 2, "2": 3, "3": 4}[kind]
            count = int(line[2:4], 16)
            payload = line[4 : 4 + 2 * count]
            addr = int(payload[: addr_len * 2], 16)
            data_hex = payload[addr_len * 2 : -2]  # drop chksum
            data = bytes.fromhex(data_hex)
            assert len(data) == count - addr_len - 1
            recs.append((addr, data))
            if min_addr is None or addr < min_addr:
                min_addr = addr
            if max_addr is None or addr + len(data) > max_addr:
                max_addr = addr + len(data)
    recs.sort()
    buf = bytearray(max_addr - min_addr)
    for addr, data in recs:
        buf[addr - min_addr : addr - min_addr + len(data)] = data
    return buf, min_addr


def emit(buf, base, out):
    # The testbench loads words in index order (from mem_*_temp[0]), so the
    # emitted stream starts at index 0 without an @addr line: the data image at
    # byte 0x40000 must not carry an address beyond the 0xFFFF-word array
    # bounds. Pads holes and the tail with 0.
    pad = (4 - (len(buf) % 4)) % 4
    buf = buf + bytes(pad)
    with open(out, "w") as f:
        for i in range(0, len(buf), 4):
            w = int.from_bytes(buf[i : i + 4], "big")
            f.write("{:08x}\n".format(w))


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    buf, base = parse_srec(sys.argv[1])
    emit(buf, base, sys.argv[2])
    return 0


if __name__ == "__main__":
    sys.exit(main())