import subprocess
import sys
import tempfile
with tempfile.TemporaryDirectory() as directory:
    subprocess.run([sys.argv[1], directory], check=True)
