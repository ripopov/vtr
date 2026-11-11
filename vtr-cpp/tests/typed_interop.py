"""Verify complete Rust/C++ files in both directions across codecs and execution modes."""
from pathlib import Path
import subprocess
import sys
import struct
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parent / "engine"))
from records_oracle import decompress
from schema_oracle import Cursor
from transactions_oracle import tagged_value


def check_value_table(path):
    data = path.read_bytes()
    assert struct.unpack_from("<HHI", data, 8) == (3, 5, 0)
    values = []
    chunks = 0
    offset = 32

    def references(value, available):
        tag, payload = value
        if tag == 25:
            assert payload < available, "forward value reference"
        elif tag == 23:
            for child in payload:
                references(child, available)
        elif tag == 24:
            for _, child in payload:
                references(child, available)

    while offset < len(data) - 24:
        kind, flags, size, crc, reserved = struct.unpack_from("<IIQII", data, offset)
        if kind == 7:
            break
        payload = data[offset + 24:offset + 24 + size]
        if kind == 11:
            assert not flags and not reserved and zlib.crc32(payload) == crc
            cursor = Cursor(decompress(payload))
            assert cursor.var() == len(values)
            count = cursor.var()
            for _ in range(count):
                entry = Cursor(cursor.take(cursor.var()))
                value = tagged_value(entry)
                entry.done()
                references(value, len(values))
                values.append(value)
            cursor.done()
            chunks += 1
        offset += 24 + size
    assert chunks == 2
    assert values == [
        (13, "FADD"),
        (23, [(25, 0), (6, 32), (11, struct.pack("<Q", 0x7ff800000000beef)), (14, bytes([0, 255]))]),
        (23, [(25, 1), (13, "late")]),
    ], values


def main():
    native, build = Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve()
    root = Path(__file__).resolve().parents[2]
    build.mkdir(parents=True, exist_ok=True)
    subprocess.run(["cargo", "build", "--locked", "--manifest-path", str(root / "Cargo.toml"),
                    "-p", "vtr", "--example", "typed_interop", "--target-dir", str(build / "rust")], check=True)
    rust = build / "rust/debug/examples/typed_interop"
    for codec in range(3):
        for mode, workers in [("inline", 0), ("threaded", 0), ("threaded", 2)]:
            for language, writer in [("cpp", native), ("rust", rust)]:
                trace = build / f"{language}-c{codec}-{mode}-w{workers}.vtr"
                subprocess.run([str(writer), "write", str(trace), str(codec), str(workers), mode], check=True)
                check_value_table(trace)
                for reader in (rust, native):
                    subprocess.run([str(reader), "read", str(trace)], check=True, stdout=subprocess.DEVNULL)
                for language, indexer in [("cpp", native), ("rust", rust)]:
                    sidecar = trace.with_suffix(f".{language}.index")
                    subprocess.run([str(indexer), "index-write", str(trace), str(sidecar)], check=True)
                    for reader in (rust, native):
                        subprocess.run([str(reader), "index-read", str(trace), str(sidecar)], check=True)
                interrupted = trace.with_suffix(".interrupted")
                interrupted.write_bytes(trace.read_bytes()[:-1])
                for language, recoverer in (("cpp", native), ("rust", rust)):
                    recovered = trace.with_suffix(f".{language}.recovered.vtr")
                    subprocess.run([str(recoverer), "recover", str(interrupted), str(recovered)], check=True)
                    for reader in (rust, native):
                        subprocess.run([str(reader), "read", str(recovered)], check=True, stdout=subprocess.DEVNULL)
                print(f"verified {trace.name} and bidirectional activity sidecars", flush=True)

    tracker = Path(sys.argv[3]).resolve()
    for mode in ("inline", "threaded"):
        for language in ("cpp", "rust"):
            trace = build / f"tracker-{language}-{mode}.vtr"
            command = [str(tracker), "--lifecycle"] if language == "cpp" else [str(rust), "tracker-write"]
            subprocess.run([*command, str(trace), mode], check=True)
            subprocess.run([str(tracker), "--read-lifecycle", str(trace)], check=True)
            subprocess.run([str(rust), "tracker-read", str(trace)], check=True)
            print(f"verified {trace.name} grouped lifetimes in both readers", flush=True)


if __name__ == "__main__":
    main()
