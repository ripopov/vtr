#!/usr/bin/env python3
"""Check complete VTR recordings with the independent Rust reader."""

import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import zlib


def checked_framing(source):
    assert source[:8] == b'\x89VTR\r\n\x1a\n'
    assert struct.unpack_from('<HHI', source, 8) == (3, 2, 0)
    offset, size, trailer = struct.unpack_from('<QQ8s', source, len(source) - 24)
    assert size == len(source) and trailer == b'VTR_END\0'
    kind, flags, length, crc, reserved = struct.unpack_from('<IIQII', source, offset)
    body = source[offset + 24:offset + 24 + length]
    assert (kind, flags, reserved) == (7, 0, 0) and zlib.crc32(body) == crc
    count, dropped = struct.unpack_from('<QQ', body)
    assert not dropped and len(body) == 16 + count * 48
    directory = bytearray(struct.pack('<Q', count))
    previous = 32
    for i in range(count):
        kind, flags, position, length, begin, end, block = struct.unpack_from('<IIQQQQQ', body, 16 + i * 48)
        assert kind in (1, 2, 3, 4, 6) and not block and position == previous
        header = struct.unpack_from('<IIQII', source, position)
        payload = source[position + 24:position + 24 + length]
        assert header[:3] == (kind, flags, length) and not header[4] and zlib.crc32(payload) == header[3]
        directory.extend(struct.pack('<IIQQQQ', kind, flags, position, length, begin, end))
        previous += 24 + length
    assert previous == offset
    return source, hashlib.sha256(source[32:offset]).hexdigest()


def main():
    if len(sys.argv) != 4:
        raise SystemExit('usage: waveform_oracle.py NATIVE RUST_ORACLE OUTPUT_DIRECTORY')
    native, rust, output = map(lambda path: Path(path).resolve(), sys.argv[1:])
    output.mkdir(parents=True, exist_ok=True)
    fixtures = subprocess.check_output([str(native), str(output)], text=True)
    rows = []
    for line in fixtures.splitlines():
        name, expected = line.split()
        recording, checksum = checked_framing((output / name).read_bytes())
        path = output / name
        actual = subprocess.check_output([str(rust), str(path)], text=True).strip()
        assert actual == expected, (name, expected, actual)
        rows.append(dict(file=name, digest=actual, unchanged_sections_sha256=checksum))
        print(f'verified {name}: {actual}')
    assert len(rows) == 12 and len({row['digest'] for row in rows}) == 1
    (output / 'oracle.json').write_text(json.dumps(dict(status='all twelve files verified', rows=rows), indent=2) + '\n')


if __name__ == '__main__':
    main()
