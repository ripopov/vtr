#!/usr/bin/env python3
"""Independently check C++ RECORD_BLOCK bytes, codecs, headers and decoded records."""
import ctypes
import ctypes.util
import json
import subprocess
import sys
from collections import defaultdict
from schema_oracle import Columns, Cursor, Row, declarations, decode, unzig

MASK = (1 << 64) - 1


def decompress(data):
    source = Cursor(data)
    codec, size = source.fixed(1), source.var()
    packed = source.take(len(source.data) - source.offset)
    if codec == 0:
        assert len(packed) == size
        return packed
    output = ctypes.create_string_buffer(size)
    if codec == 1:
        library = ctypes.CDLL(ctypes.util.find_library("lz4"))
        function = library.LZ4_decompress_safe
        function.argtypes = (ctypes.c_char_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int)
        function.restype = ctypes.c_int
        assert function(packed, output, len(packed), size) == size
    elif codec == 2:
        library = ctypes.CDLL(ctypes.util.find_library("zstd"))
        function = library.ZSTD_decompress
        function.argtypes = (ctypes.c_void_p, ctypes.c_size_t, ctypes.c_char_p, ctypes.c_size_t)
        function.restype = ctypes.c_size_t
        assert function(output, size, packed, len(packed)) == size
    else:
        raise AssertionError(codec)
    return output.raw


def decode_fixture(fixture):
    types = declarations(bytes.fromhex(fixture["declarations"]))
    generators = {id: (kind, type_id) for id, kind, type_id in fixture["generators"]}
    expected = {}
    expected_order = defaultdict(list)
    for id, gen, begin, end, parent, payload in fixture["rows"]:
        source = Row(bytes.fromhex(payload))
        value = decode(types, generators[gen][1], source)
        source.done()
        expected[id] = (gen, begin, end, parent, value)
        expected_order[gen].append(id)
    r = Cursor(bytes.fromhex(fixture["block"]))
    number, count, min_id, max_id, t_min, t_max = [r.fixed(8) for _ in range(6)]
    assert number == 7
    n_segments, n_pages = r.fixed(4), r.fixed(4)
    segments = [tuple(r.fixed(4) for _ in range(5)) + tuple(r.fixed(8) for _ in range(6)) for _ in range(n_segments)]
    pages = [tuple(r.fixed(4) for _ in range(3)) for _ in range(n_pages)]
    blobs = r.take(len(r.data) - r.offset)
    actual, actual_order = {}, defaultdict(list)
    blob_end = 0
    for page_index, (blob_offset, blob_length, raw_length) in enumerate(pages):
        assert blob_offset == blob_end
        blob_end += blob_length
        raw = decompress(blobs[blob_offset:blob_end])
        assert len(raw) == raw_length
        offset = 0
        for gen, page, n, raw_offset, length, seg_min, seg_max, parent_min, parent_max, begin_min, end_max in segments:
            if page != page_index:
                continue
            assert raw_offset == offset
            offset += length
            source = Cursor(raw[raw_offset:offset])
            assert source.var() == 9
            lengths = [source.var() for _ in range(9)]
            streams = [source.take(size) for size in lengths]
            source.done()
            cols = [Cursor(stream) for stream in streams[:4]]
            args = Columns([stream.hex() for stream in streams[4:]])
            kind, type_id = generators[gen]
            previous_id = previous_parent = previous_end = 0
            rows = []
            for _ in range(n):
                begin = (previous_end + unzig(cols[0].var())) & MASK
                id = (previous_id + unzig(cols[1].var())) & MASK
                if kind == 4:
                    distance = cols[2].var()
                    assert distance == 0 or distance < id
                    parent = id - distance if distance else 0
                else:
                    parent = (previous_parent + unzig(cols[2].var())) & MASK
                    assert parent != 0
                assert id != 0 and parent < id
                duration = cols[3].var() if kind == 3 else 0
                end = begin + duration
                assert end <= MASK
                value = decode(types, type_id, args)
                assert id not in actual
                actual[id] = (gen, begin, end, parent, value)
                actual_order[gen].append(id)
                rows.append((id, begin, end, parent))
                previous_id, previous_parent, previous_end = id, parent, end
            for column in cols:
                column.done()
            args.done()
            assert (min(row[0] for row in rows), max(row[0] for row in rows)) == (seg_min, seg_max)
            assert (min(row[1] for row in rows), max(row[2] for row in rows)) == (begin_min, end_max)
            parents = [row[3] for row in rows if row[3] != 0]
            assert (min(parents, default=0), max(parents, default=0)) == (parent_min, parent_max)
        assert offset == raw_length
    assert blob_end == len(blobs)
    assert actual == expected
    assert actual_order == expected_order
    assert len(actual) == count
    assert (min(actual), max(actual)) == (min_id, max_id)
    assert (min(row[1] for row in actual.values()), max(row[2] for row in actual.values())) == (t_min, t_max)


def main():
    for codec in range(3):
        fixture = json.loads(subprocess.check_output([sys.argv[1], "--fixture", str(codec)], text=True))
        decode_fixture(fixture)
    print("Independent Python decoder checked all record headers/columns, page partitions, occurrence/order identity, full-u64 fields and stored/LZ4/Zstd codecs")


if __name__ == "__main__":
    main()
