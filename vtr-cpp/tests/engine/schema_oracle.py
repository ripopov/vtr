#!/usr/bin/env python3
"""Decode C++ datatype and argument fixtures independently, preserving float bits."""
import json
import struct
import subprocess
import sys


class Cursor:
    def __init__(self, data):
        self.data = memoryview(data)
        self.offset = 0

    def take(self, count):
        if count < 0 or count > len(self.data) - self.offset:
            raise ValueError("truncated bytes")
        result = self.data[self.offset:self.offset + count].tobytes()
        self.offset += count
        return result

    def fixed(self, size, signed=False):
        return int.from_bytes(self.take(size), "little", signed=signed)

    def var(self):
        value = 0
        for i in range(10):
            b = self.fixed(1)
            value |= (b & 127) << (i * 7)
            if b < 128:
                if value >= 1 << 64 or (i and b == 0):
                    raise ValueError("noncanonical/overflowing varint")
                return value
        raise ValueError("overflowing varint")

    def text(self):
        return self.take(self.var()).decode("utf-8")

    def done(self):
        assert self.offset == len(self.data), (self.offset, len(self.data))


def unzig(value):
    return (value >> 1) ^ -(value & 1)


def declarations(data):
    source = Cursor(data)
    first, count = source.var(), source.var()
    assert first == 23
    types = [("scalar", i) for i in range(23)]
    for _ in range(count):
        kind, name = source.fixed(1), source.text()
        if kind == 1:
            fields = [(source.text(), source.var()) for _ in range(source.var())]
            assert len(set(key for key, _ in fields)) == len(fields)
            assert all(type_id < len(types) for _, type_id in fields)
            item = ("struct", name, fields)
        else:
            element = source.var()
            assert element < len(types)
            if kind == 2:
                item = ("array", name, element, source.var())
            elif kind == 3:
                item = ("sequence", name, element)
            elif kind == 4:
                values = [(source.fixed(8), source.text()) for _ in range(source.var())]
                item = ("enum", name, element, values)
            else:
                raise AssertionError(kind)
        types.append(item)
    source.done()
    return types


WIDTHS = {2: 1, 3: 2, 4: 4, 5: 8, 6: 1, 7: 2, 8: 4, 9: 8, 10: 4, 11: 8, 12: 4, 18: 8, 20: 8}


class Row:
    def __init__(self, data):
        self.source = Cursor(data)

    def count(self):
        return self.source.fixed(4)

    def scalar(self, tag):
        r = self.source
        if tag == 0:
            return None
        if tag == 1:
            value = r.fixed(1)
            assert value <= 1
            return bool(value)
        if tag in WIDTHS:
            if tag in (10, 11):
                return (tag, r.take(WIDTHS[tag]).hex())
            return r.fixed(WIDTHS[tag], signed=tag in (2, 3, 4, 5))
        if tag in (13, 14):
            data = r.take(r.fixed(4))
            return data.decode("utf-8") if tag == 13 else data
        if tag in (15, 16, 17):
            width = r.fixed(8)
            per_byte = {15: 8, 16: 4, 17: 2}[tag]
            return (width, r.take((width + per_byte - 1) // per_byte))
        if tag in (21, 22):
            return (r.fixed(8, signed=tag == 21), r.fixed(8, signed=True))
        raise AssertionError(tag)

    def done(self):
        self.source.done()


class Columns:
    def __init__(self, streams, shared_text=()):
        self.numeric, self.floating, self.text, dictionary, self.misc = [Cursor(bytes.fromhex(s)) for s in streams]
        descriptor = dictionary.var()
        self.shared = bool(descriptor & 1)
        self.shared_text = shared_text
        self.dictionary = [dictionary.text() for _ in range(descriptor >> 1)]
        assert len(self.dictionary) == len(set(self.dictionary))
        dictionary.done()
        self.next_text = 0

    def count(self):
        result = self.numeric.var()
        assert result <= (1 << 32) - 1
        return result

    def scalar(self, tag):
        if tag == 0:
            return None
        if tag == 1:
            value = self.numeric.fixed(1)
            assert value <= 1
            return bool(value)
        if tag in (2, 3, 4, 5):
            result = unzig(self.numeric.var())
            bits = WIDTHS[tag] * 8
            assert -(1 << (bits - 1)) <= result < 1 << (bits - 1)
            return result
        if tag in (6, 7, 8, 9, 18, 20):
            result = self.numeric.var()
            assert result < 1 << (WIDTHS[tag] * 8)
            return result
        if tag in (10, 11):
            return (tag, self.floating.take(WIDTHS[tag]).hex())
        if tag == 12:
            return self.misc.var()
        if tag == 13:
            index = self.text.var()
            if self.shared:
                global_reference = index & 1
                index >>= 1
                if global_reference:
                    assert index < len(self.shared_text)
                    return self.shared_text[index]
            assert index < len(self.dictionary) and index <= self.next_text
            self.next_text += index == self.next_text
            return self.dictionary[index]
        if tag == 14:
            return self.misc.take(self.misc.var())
        if tag in (15, 16, 17):
            width = self.misc.var()
            per_byte = {15: 8, 16: 4, 17: 2}[tag]
            return (width, self.misc.take((width + per_byte - 1) // per_byte))
        if tag in (21, 22):
            raw = self.numeric.var()
            return (unzig(raw) if tag == 21 else raw, unzig(self.numeric.var()))
        raise AssertionError(tag)

    def done(self):
        for r in (self.numeric, self.floating, self.text, self.misc):
            r.done()
        assert self.next_text == len(self.dictionary)


def decode(types, id, source):
    item = types[id]
    kind = item[0]
    if kind == "scalar":
        return source.scalar(item[1])
    if kind == "struct":
        return {key: decode(types, field_type, source) for key, field_type in item[2]}
    if kind in ("array", "sequence"):
        count = item[3] if kind == "array" else source.count()
        return [decode(types, item[2], source) for _ in range(count)]
    if kind == "enum":
        value = decode(types, item[2], source)
        literal = next((label for bits, label in item[3] if bits == value % (1 << 64)), None)
        return {"value": value, "literal": literal}
    raise AssertionError(kind)


def check_fixture(flag):
    fixture = json.loads(subprocess.check_output([sys.argv[1], flag], text=True))
    types = declarations(bytes.fromhex(fixture["declarations"]))
    columns = Columns(fixture["columns"], fixture["global_text"])
    requests = []
    for index, (type_id, payload) in enumerate(fixture["rows"]):
        row = Row(bytes.fromhex(payload))
        expected = decode(types, type_id, row)
        row.done()
        actual = decode(types, type_id, columns)
        assert actual == expected, (type_id, expected, actual)
        if index < 3:
            requests.append(actual)
    columns.done()
    for i, request in enumerate(requests):
        assert request["address"] == (1 << 64) - 1 - i
        assert request["operation"] == {"value": [-7, 9, 22][i], "literal": ["load", "store", None][i]}
        assert request["operands"][0] == {"index": 0, "offset": -32768, "weight": (10, "00000080")}
        assert request["operands"][1]["weight"] == (10, "efbec07f")
        assert request["extra"] == [request["operands"][2]] * i
        assert request["description"] == ("retry Δ" if i < 2 else "")
        assert request["data"] == bytes([0, 255, i])
        assert request["estimate"] == (11, "beba00000000f87f")
        assert request["flags"] == [bool(i % 2)] * (i + 1)
    print(f"Independent Python decoder validated {len(fixture['rows'])} mixed typed payloads and {len(types) - 23} shared declarations")


if __name__ == "__main__":
    check_fixture("--fixture")
    check_fixture("--shared-fixture")
