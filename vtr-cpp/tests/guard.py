import subprocess
import sys
import tempfile
from pathlib import Path

with tempfile.TemporaryDirectory() as directory:
    for mode, code, ending in [("normal", 0, "0 0 0 100"), ("exit", 7, "2 7 0 100"),
                               ("abort", -6, "3 0 0 100"), ("inside", -6, "3 0 1 100"),
                               ("stop", 0, "1 0 0 100"), ("deaf", -15, "1 0 0 100"),
                               ("stop-inside", -15, "1 0 0 100")]:
        path = Path(directory) / f"{mode}.vtr"
        result = subprocess.run([sys.argv[1], mode, str(path)], timeout=5)
        assert result.returncode == code, (mode, result.returncode)
        actual = subprocess.check_output([sys.argv[1], "check", str(path)], text=True).strip()
        assert actual == ending, (mode, actual, ending)
