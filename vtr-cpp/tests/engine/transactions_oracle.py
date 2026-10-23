#!/usr/bin/env python3
"""Independently decode all TX_BLOCK columns and compare C++ source records."""
import json
import subprocess
import sys
from schema_oracle import Cursor, Columns, Row, decode, declarations, unzig
from records_oracle import decompress, MASK


def payload(tag, numeric, floating, string, misc):
    if tag == 0:
        return None
    if tag == 1:
        value = numeric.fixed(1)
        assert value <= 1
        return bool(value)
    if 2 <= tag <= 5:
        value = unzig(numeric.var())
        bits = 8 << (tag - 2)
        assert -(1 << (bits - 1)) <= value < 1 << (bits - 1)
        return value
    if 6 <= tag <= 9:
        value = numeric.var()
        assert value < 1 << (8 << (tag - 6))
        return value
    if tag in (10, 11):
        return floating.take(4 if tag == 10 else 8)
    if tag == 12:
        return string.var()
    if tag in (13, 14):
        value = misc.take(misc.var())
        return value.decode("utf-8") if tag == 13 else value
    if tag in (15, 16, 17):
        width = misc.var()
        per_byte = {15: 8, 16: 4, 17: 2}[tag]
        return width, misc.take((width + per_byte - 1) // per_byte)
    if tag in (18, 20):
        return numeric.var()
    if tag == 19:
        return unzig(numeric.var()), string.var()
    if tag in (21, 22):
        value = numeric.var()
        return (unzig(value) if tag == 21 else value), unzig(numeric.var())
    if tag == 23:
        return [tagged_value(misc) for _ in range(misc.var())]
    if tag == 24:
        return attrs(misc)
    raise AssertionError(tag)


def tagged_value(cursor):
    tag = cursor.fixed(1)
    return tag, payload(tag, cursor, cursor, cursor, cursor)


def attrs(cursor):
    result = [(cursor.var(), tagged_value(cursor)) for _ in range(cursor.var())]
    assert len(set(key for key, _ in result)) == len(result)
    return result


class AttributeColumns:
    def __init__(self, c):
        self.c = c

    def read(self):
        c = self.c
        result = []
        for _ in range(c[6].var()):
            key, tag = c[7].var(), c[8].fixed(1)
            result.append((key, (tag, payload(tag, c[9], c[10], c[11], c[12]))))
        assert len(set(key for key, _ in result)) == len(result)
        return result


def canonical_transaction(raw, types, generators):
    r = Cursor(bytes.fromhex(raw))
    id, parent, begin, end = [r.fixed(8) for _ in range(4)]
    generator, status, kind = r.fixed(4), r.fixed(1), r.fixed(1)
    row = Row(r.take(r.var()))
    arguments = decode(types, generators[generator][1], row)
    row.done()
    attributes = attrs(r)
    events = []
    for _ in range(r.var()):
        event_id, time, name = r.fixed(8), r.fixed(8), r.fixed(4)
        events.append((event_id, time, name, attrs(r)))
    stages = []
    for _ in range(r.var()):
        stage_id, name, lane, start = r.fixed(8), r.fixed(4), r.fixed(4), r.fixed(8)
        finish = r.fixed(8) if r.fixed(1) else None
        stages.append((stage_id, name, lane, start, finish, attrs(r)))
    blocks = [r.fixed(8) for _ in range(r.var())]
    r.done()
    return id, (parent, begin, end, generator, status, kind, arguments, attributes, events, stages, blocks)


def canonical_relation(raw):
    r = Cursor(bytes.fromhex(raw))
    kind, source, destination = r.fixed(4), r.fixed(8), r.fixed(8)
    result = kind, source, destination, attrs(r)
    r.done()
    return result


def stage_end(r, begin):
    if r.data[r.offset:r.offset + 10].tobytes() == bytes([0x80] * 9 + [2]):
        r.take(10)
        assert begin == 0
        return MASK
    value = r.var()
    if value == 0:
        return None
    result = begin + value - 1
    assert result <= MASK
    return result


def bounds(transactions, relations):
    parents = [t[0] for t in transactions.values() if t[0]]
    embedded = [e[0] for t in transactions.values() for e in [*t[8], *t[9]]]
    return (len(transactions), len(relations), min(transactions, default=0), max(transactions, default=0),
            min((t[1] for t in transactions.values()), default=0), max((t[2] for t in transactions.values()), default=0),
            min(parents, default=0), max(parents, default=0), min(embedded, default=0), max(embedded, default=0),
            min((r[1] for r in relations), default=0), max((r[1] for r in relations), default=0),
            min((r[2] for r in relations), default=0), max((r[2] for r in relations), default=0))


def fixture(data):
    types = declarations(bytes.fromhex(data["declarations"]))
    generators = {id: (kind, type_id) for id, kind, type_id in data["generators"]}
    expected = dict(canonical_transaction(row, types, generators) for row in data["transactions"])
    expected_relations = [canonical_relation(row) for row in data["relations"]]
    source = Cursor(bytes.fromhex(data["block"]))
    number = source.fixed(8)
    block_bounds = tuple(source.fixed(8) for _ in range(14))
    assert number == 7 and block_bounds == bounds(expected, expected_relations)
    generator_count, page_count = source.fixed(4), source.fixed(4)
    table = [source.fixed(4) for _ in range(generator_count)]
    assert table == sorted(set(t[3] for t in expected.values()))
    pages = [(tuple(source.fixed(8) for _ in range(14)), tuple(source.fixed(4) for _ in range(3))) for _ in range(page_count)]
    blobs = source.take(len(source.data) - source.offset)
    actual, actual_relations = {}, []
    previous_page_id = 0
    blob_end = 0
    for header, (offset, length, raw_length) in pages:
        assert offset == blob_end
        blob_end += length
        raw = decompress(blobs[offset:blob_end])
        assert len(raw) == raw_length
        r = Cursor(raw)
        assert r.var() == 33
        lengths = [r.var() for _ in range(33)]
        streams = [r.take(n) for n in lengths]
        r.done()
        c = [Cursor(stream) for stream in streams]
        attributes = AttributeColumns(c)
        arguments = Columns([stream.hex() for stream in streams[24:29]], data["global_text"])
        id = start = event_id = stage_id = from_id = 0
        transactions, relations = {}, []
        for _ in range(header[0]):
            id = (id + unzig(c[0].var())) & MASK
            generator = table[c[1].var()]
            start = (start + unzig(c[2].var())) & MASK
            end = start + c[3].var()
            classification = c[4].fixed(1)
            distance = c[5].var()
            parent = id - distance if distance else 0
            tx_attrs = attributes.read()
            events, stages = [], []
            for _ in range(c[13].var()):
                time = (start + unzig(c[14].var())) & MASK
                name = c[15].var()
                event_id = (event_id + unzig(c[31].var())) & MASK
                events.append((event_id, time, name, attributes.read()))
            for _ in range(c[16].var()):
                name, lane = c[17].var(), c[18].var()
                begin = (start + unzig(c[19].var())) & MASK
                end_stage = stage_end(c[20], begin)
                stage_id = (stage_id + unzig(c[32].var())) & MASK
                stages.append((stage_id, name, lane, begin, end_stage, attributes.read()))
            args = decode(types, generators[generator][1], arguments)
            blocks, block = [], 0
            for _ in range(c[29].var()):
                delta = c[30].var()
                assert delta > 0
                block += delta
                blocks.append(block)
            assert id not in actual and id > previous_page_id
            previous_page_id = id
            transactions[id] = (parent, start, end, generator, classification & 15, classification >> 4, args, tx_attrs, events, stages, blocks)
        for _ in range(header[1]):
            kind = c[21].var()
            from_id = (from_id + unzig(c[22].var())) & MASK
            to_id = (from_id + unzig(c[23].var())) & MASK
            relations.append((kind, from_id, to_id, attributes.read()))
        assert header == bounds(transactions, relations)
        actual.update(transactions)
        actual_relations.extend(relations)
        for i, column in enumerate(c):
            if not 24 <= i <= 28:
                column.done()
        arguments.done()
    assert blob_end == len(blobs)
    assert actual == expected and actual_relations == expected_relations
    first = actual[1]
    assert first[7][2][1] == (2, -1)
    assert first[7][10][1] == (10, bytes.fromhex("efbec07f"))
    assert first[8][0][1] == MASK
    assert first[9][0][4] == MASK and first[9][1][4] is None
    assert first[10] == [1, 4, 100]


def main():
    for codec in range(3):
        data = json.loads(subprocess.check_output([sys.argv[1], "--fixture", str(codec)], text=True))
        fixture(data)
    print("Independent Python decoder validated all 33 TX columns, every attribute tag, embedded IDs, full-domain/open stage ends, page/parent/endpoint bounds, and all three codecs")


if __name__ == "__main__":
    main()
