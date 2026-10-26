"""Verify complete Rust/C++ files in both directions across codecs and execution modes."""
from pathlib import Path
import subprocess
import sys


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
