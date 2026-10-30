"""Verify deferred printf messages byte for byte in both language readers."""
from pathlib import Path
import subprocess
import sys

native, build = Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve()
root = Path(__file__).resolve().parents[2]
build.mkdir(parents=True, exist_ok=True)
subprocess.run([str(native), str(build)], check=True)
subprocess.run(["cargo", "build", "--locked", "--manifest-path", str(root / "Cargo.toml"),
                "-p", "vtr", "--example", "printf_interop"], check=True)
rust = root / "target/debug/examples/printf_interop"
for mode in ("inline", "threaded"):
    for prefix in ("", "sample-"):
        subprocess.run([str(rust), "read", str(build / f"{prefix}{mode}.vtr")], check=True)
    trace = build / f"rust-{mode}.vtr"
    subprocess.run([str(rust), "write", str(trace), mode], check=True)
    for reader, command in ((rust, "read"), (native, "--read")):
        subprocess.run([str(reader), command, str(trace)], check=True)
