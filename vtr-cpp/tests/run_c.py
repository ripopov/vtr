import subprocess
import sys
import tempfile
from pathlib import Path

with tempfile.TemporaryDirectory() as directory:
    subprocess.run([sys.argv[1], str(Path(directory) / "trace.vtr")], check=True)
