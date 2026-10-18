#!/usr/bin/env python3
"""Compare independent decoders on Rust and C++ files with every supported codec."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

here = Path(__file__).resolve().parent
native = Path(sys.argv[1]).resolve()
manifest = here / "rust" / "Cargo.toml"
env = dict(os.environ, CARGO_TARGET_DIR=str(native.parent / "rust-oracle"))
subprocess.run(["cargo", "build", "--locked", "--manifest-path", str(manifest)], env=env, check=True)
oracle = native.parent / "rust-oracle" / "debug" / "vtr-cpp-oracle"
with tempfile.TemporaryDirectory() as directory:
    for writer in (native, oracle):
        for codec in range(3):
            path = Path(directory) / f"{writer.name}-{codec}.vtr"
            subprocess.run([str(writer), "write", str(path), str(codec)], check=True, capture_output=True)
            expected = subprocess.check_output([str(oracle), "digest", str(path)], text=True).strip()
            actual = subprocess.check_output([str(native), "digest", str(path)], text=True).strip()
            assert actual == expected, (writer.name, codec, actual, expected)
            print(f"verified {writer.name}, codec {codec}: {actual}")
            if codec == 1:
                for index_writer in (native, oracle):
                    subprocess.run([str(index_writer), "index", str(path)], check=True, stdout=subprocess.DEVNULL)
                    expected = subprocess.check_output([str(oracle), "activity", str(path)], text=True)
                    actual = subprocess.check_output([str(native), "activity", str(path)], text=True)
                    assert actual == expected, (writer.name, index_writer.name, actual, expected)
                print("verified both activity sidecar implementations")

    path = Path(directory) / "long-gap.vtr"
    subprocess.run([str(oracle), "long-gap", str(path)], check=True)
    subprocess.run([str(oracle), "index", str(path)], check=True, stdout=subprocess.DEVNULL)
    expected = subprocess.check_output([str(oracle), "activity", str(path)], text=True)
    actual = subprocess.check_output([str(native), "activity", str(path)], text=True)
    assert actual == expected, ("saturated activity gaps", actual, expected)

    tracked = subprocess.check_output([str(native.parent / "vtr_track_test"), directory], text=True)
    diagnostics = Path(directory) / "tracking.txt"
    diagnostics.write_text(tracked)
    subprocess.run([str(oracle), "track", directory, str(diagnostics)], check=True)
    for path in Path(directory).glob("track_*.vtr"):
        expected = subprocess.check_output([str(oracle), "digest", str(path)], text=True).strip()
        actual = subprocess.check_output([str(native), "digest", str(path)], text=True).strip()
        assert actual == expected, (path.name, actual, expected)
    print("verified tracker and SystemVerilog/DPI recordings")
