#!/usr/bin/env python3
"""Decode sidecar rows independently and compare them with producer changes."""
import pathlib
import struct
import subprocess
import sys
import zlib
from records_oracle import decompress
from schema_oracle import Cursor

root = pathlib.Path(sys.argv[2])
subprocess.run([sys.argv[1], str(root)], check=True, capture_output=True)
for name in ("identity-0", "windows"):
    trace = (root / f"{name}.vtr").read_bytes()
    sidecar = (root / f"{name}.index").read_bytes()
    assert trace[:8] == b"\x89VTR\r\n\x1a\n"
    assert struct.unpack_from("<HHI", trace, 8) == (3, 4, 0)
    assert any(trace[16:32])
    trace_directory = struct.unpack_from("<Q", trace, len(trace) - 24)[0]
    trace_length = struct.unpack_from("<Q", trace, trace_directory + 8)[0]
    fingerprint = (len(trace), zlib.crc32(trace[trace_directory + 24:trace_directory + 24 + trace_length]), trace[16:32])
    assert sidecar[:8] == b"\x89VTI\r\n\x1a\n"
    assert struct.unpack_from("<HH", sidecar, 8) == (3, 0)
    assert sidecar[12:32] == bytes(20)
    offset, sections = 32, []
    starts, stretches, signals, minimum = {}, {}, None, None
    while offset < len(sidecar) - 24:
        kind, flags, length, crc, reserved = struct.unpack_from("<IIQII", sidecar, offset)
        body = sidecar[offset + 24:offset + 24 + length]
        assert len(body) == length and not flags and not reserved and zlib.crc32(body) == crc
        begin = end = 0
        if kind == 1:
            size, directory_crc, source, signals = struct.unpack_from("<QIB3xI", body)
            minimum = struct.unpack_from("<Q", body, 32)[0]
            assert len(body) == 56 and source == 1 and (size, directory_crc, body[40:56]) == fingerprint
        elif kind in (2, 3):
            if kind == 2:
                begin, end, source_bytes, rows, closed, exponent = struct.unpack_from("<QQQIIB", body)
                assert exponent == 0 and source_bytes >= 24 and begin <= end and body[33:40] == bytes(7)
                source, count = Cursor(decompress(body[40:])), 6
            else:
                rows, reserved = struct.unpack_from("<II", body)
                assert not reserved
                source, count = Cursor(decompress(body[8:])), 3
            lengths = [source.var() for _ in range(count)]
            columns = [Cursor(source.take(n)) for n in lengths]
            source.done()
            signal, observed = 0, 0
            for row in range(rows):
                delta = columns[0].var()
                assert row == 0 or delta > 0
                signal += delta
                assert signal < signals
                if kind == 2:
                    head = columns[1].var()
                    if head & 1:
                        assert signal not in starts
                        starts[signal] = begin + columns[2].var()
                        assert minimum < starts[signal] <= end
                    for _ in range(head >> 1):
                        length, silence, gap = (columns[i].var() for i in (3, 4, 5))
                        first, last = starts[signal], starts[signal] + length
                        stretches.setdefault(signal, []).append((first, last, gap))
                        starts[signal] = last + silence
                        assert silence > 1 and begin <= starts[signal] <= end
                    observed += head >> 1
                else:
                    length, gap = columns[1].var(), columns[2].var()
                    first = starts.pop(signal)
                    stretches.setdefault(signal, []).append((first, first + length, gap))
            if kind == 2:
                assert observed == closed
            for column in columns:
                column.done()
        elif kind == 7:
            assert struct.unpack_from("<Q", body)[0] == len(sections)
            assert len(body) == 8 + 40 * len(sections)
            for i, section in enumerate(sections):
                assert struct.unpack_from("<IIQQQQ", body, 8 + 40 * i) == section
            assert struct.unpack_from("<QQ", sidecar, len(sidecar) - 24) == (offset, len(sidecar))
        else:
            raise AssertionError(kind)
        if kind != 7:
            sections.append((kind, flags, offset, len(body), begin, end))
        offset += 24 + len(body)
    assert not starts and offset == len(sidecar) - 24
    assert sidecar[-8:] == b"VTR_END\x00"
    expanded = {}
    for signal, runs in stretches.items():
        expanded[signal] = []
        for first, last, gap in runs:
            assert (first == last and gap == 0) or (first < last and gap == 1)
            expanded[signal].extend(range(first, last + 1))
    wanted = {0: [1]} if name == "identity-0" else {s: [t for t in range(6, 260) if (t * (s + 3) + s * s) % (s * 7 + 3) < s + 1] for s in range(5)}
    assert expanded == wanted
print("independent activity fingerprint and producer oracle passed")
