#!/usr/bin/env python3
"""Check Cargo's source-package selection and independently packageable archives."""
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[2]
metadata = json.loads(subprocess.check_output(
    ['cargo', 'metadata', '--no-deps', '--format-version', '1'], cwd=ROOT, timeout=60))
for package in metadata['packages']:
    source = Path(package['manifest_path']).parent
    files = subprocess.check_output(
        ['cargo', 'package', '--offline', '--locked', '--allow-dirty', '--list', '-p', package['name']],
        cwd=ROOT, text=True, timeout=120).splitlines()
    for name in ('LICENSE-MIT', 'LICENSE-APACHE'):
        assert name in files, f"{package['name']}: missing {name}"
        assert (source / name).read_bytes() == (ROOT / name).read_bytes()
    # Cargo cannot publish path-only dependencies without versions. Verify real
    # .crate archives for packages that do not require publishing sibling crates.
    if not any(d.get('path') for d in package['dependencies']):
        with tempfile.TemporaryDirectory(prefix='vtr-cargo-license-') as directory:
            subprocess.run(['cargo', 'package', '--offline', '--locked', '--allow-dirty', '--no-verify',
                            '--target-dir', directory, '-p', package['name']],
                           cwd=ROOT, check=True, timeout=120)
            archive_path = Path(directory) / 'package' / f"{package['name']}-{package['version']}.crate"
            with tarfile.open(archive_path) as archive:
                for name in ('LICENSE-MIT', 'LICENSE-APACHE'):
                    entry = archive.extractfile(f"{package['name']}-{package['version']}/{name}")
                    assert entry.read() == (ROOT / name).read_bytes()
    print(f"Verified Cargo source licenses: {package['name']}")
