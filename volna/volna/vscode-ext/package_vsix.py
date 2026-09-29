#!/usr/bin/env python3
"""Build and verify a VS Code package for this machine's native platform."""

import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import struct
import subprocess
import tempfile
import xml.etree.ElementTree as ET
import zipfile


EXT = Path(__file__).resolve().parent
ROOT = EXT.parents[2]
VSCE_VERSION = "4.0.0"
MEDIA = ("theme.mjs", "volna.js", "volna_bg.wasm")


def host_target():
    system = platform.system()
    machine = platform.machine().lower()
    arch = {"x86_64": "x64", "amd64": "x64", "aarch64": "arm64", "arm64": "arm64"}.get(machine)
    os_name = {"Linux": "linux", "Darwin": "darwin", "Windows": "win32"}.get(system)
    if not os_name or not arch:
        raise ValueError(f"unsupported packaging host: {system} {machine}")
    if os_name == "linux" and platform.libc_ver()[0] != "glibc":
        raise ValueError("Linux packaging requires glibc; Alpine needs its own target")
    return f"{os_name}-{arch}"


def check_binary(data, target):
    os_name, arch = target.split("-")
    if os_name == "linux":
        if len(data) < 20 or data[:6] != b"\x7fELF\x02\x01":
            raise ValueError("expected a 64-bit little-endian ELF server")
        actual = struct.unpack_from("<H", data, 18)[0]
        expected = {"x64": 62, "arm64": 183}[arch]
    elif os_name == "darwin":
        if len(data) < 8 or data[:4] != b"\xcf\xfa\xed\xfe":
            raise ValueError("expected a 64-bit little-endian Mach-O server")
        actual = struct.unpack_from("<I", data, 4)[0]
        expected = {"x64": 0x01000007, "arm64": 0x0100000C}[arch]
    else:
        if len(data) < 64 or data[:2] != b"MZ":
            raise ValueError("expected a Windows PE server")
        offset = struct.unpack_from("<I", data, 0x3C)[0]
        if offset + 6 > len(data) or data[offset:offset + 4] != b"PE\0\0":
            raise ValueError("invalid Windows PE header")
        actual = struct.unpack_from("<H", data, offset + 4)[0]
        expected = {"x64": 0x8664, "arm64": 0xAA64}[arch]
    if actual != expected:
        raise ValueError(f"server architecture {actual:#x} does not match {target} ({expected:#x})")


def verify_vsix(vsix, target, server):
    with zipfile.ZipFile(vsix) as archive:
        files = set(archive.namelist())
        manifest = ET.fromstring(archive.read("extension.vsixmanifest"))
        ns = {"v": "http://schemas.microsoft.com/developer/vsx-schema/2011"}
        identity = manifest.find("v:Metadata/v:Identity", ns)
        if identity is None or identity.get("TargetPlatform") != target:
            raise ValueError(f"VSIX targetPlatform is not {target}")
        package = json.loads(archive.read("extension/package.json"))
        if identity.get("Version") != package["version"]:
            raise ValueError("VSIX version does not match package.json")
        name = "volna-server.exe" if target.startswith("win32-") else "volna-server"
        bundled = {item for item in files if item.startswith("extension/bin/") and not item.endswith("/")}
        if bundled != {f"extension/bin/{name}"}:
            raise ValueError(f"VSIX contains unexpected servers: {sorted(bundled)}")
        binary = archive.read(f"extension/bin/{name}")
        check_binary(binary, target)
        if binary != server.read_bytes():
            raise ValueError("VSIX server differs from the native build")
        for item in MEDIA:
            if f"extension/media/{item}" not in files or not archive.getinfo(f"extension/media/{item}").file_size:
                raise ValueError(f"VSIX is missing media/{item}")


def package(target):
    for item in MEDIA:
        if not (EXT / "media" / item).is_file():
            raise ValueError(f"missing media/{item}; run web/build.sh first")
    subprocess.run(["cargo", "build", "--locked", "-p", "volna-server", "--profile", "viewer"], cwd=ROOT, check=True)
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=ROOT))
    name = "volna-server.exe" if target.startswith("win32-") else "volna-server"
    built = Path(metadata["target_directory"]) / "viewer" / name
    check_binary(built.read_bytes(), target)
    help_result = subprocess.run([str(built), "--help"], capture_output=True, text=True, check=True, timeout=10)
    if "Usage: volna-server" not in help_result.stdout:
        raise ValueError("native server did not start correctly")
    bin_dir = EXT / "bin"
    bin_dir.mkdir(exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=bin_dir, prefix=".volna-server-", delete=False) as temp:
        temp_path = Path(temp.name)
    try:
        shutil.copy2(built, temp_path)
        os.replace(temp_path, bin_dir / name)
    finally:
        temp_path.unlink(missing_ok=True)
    other = "volna-server" if name.endswith(".exe") else "volna-server.exe"
    (bin_dir / other).unlink(missing_ok=True)
    version = json.loads((EXT / "package.json").read_text())["version"]
    vsix = EXT / f"volna-{version}-{target}.vsix"
    local_vsce = os.environ.get("VOLNA_VSCE")
    if local_vsce:
        command = [local_vsce]
    else:
        npx = "npx.cmd" if os.name == "nt" else "npx"
        command = [npx, "--yes", f"@vscode/vsce@{VSCE_VERSION}"]
    subprocess.run([*command, "package", "--no-dependencies", "--target", target, "--out", str(vsix)], cwd=EXT, check=True)
    verify_vsix(vsix, target, bin_dir / name)
    print(f"Verified {vsix}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", help="expected VS Code target; must match this build host")
    args = parser.parse_args()
    actual_target = host_target()
    if args.target and args.target != actual_target:
        parser.error(f"{args.target} does not match the build host {actual_target}")
    package(actual_target)
