"""Headless checks for platform tagging and packaged server selection."""

import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest
import zipfile


spec = importlib.util.spec_from_file_location("package_vsix", Path(__file__).with_name("package_vsix.py"))
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)


def binary(target):
    os_name, arch = target.split("-")
    if os_name == "linux":
        data = bytearray(64)
        data[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", data, 18, {"x64": 62, "arm64": 183}[arch])
    elif os_name == "darwin":
        data = bytearray(64)
        data[:4] = b"\xcf\xfa\xed\xfe"
        struct.pack_into("<I", data, 4, {"x64": 0x01000007, "arm64": 0x0100000C}[arch])
    else:
        data = bytearray(128)
        data[:2] = b"MZ"
        struct.pack_into("<I", data, 0x3C, 80)
        data[80:84] = b"PE\0\0"
        struct.pack_into("<H", data, 84, {"x64": 0x8664, "arm64": 0xAA64}[arch])
    return bytes(data)


class PackageTests(unittest.TestCase):
    def test_binary_architectures(self):
        for os_name in ("linux", "darwin", "win32"):
            for arch in ("x64", "arm64"):
                target = f"{os_name}-{arch}"
                with self.subTest(target=target):
                    packager.check_binary(binary(target), target)
                    wrong = f"{os_name}-{'arm64' if arch == 'x64' else 'x64'}"
                    with self.assertRaisesRegex(ValueError, "does not match"):
                        packager.check_binary(binary(target), wrong)

    def test_vsix_rejects_untagged_or_wrong_server(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            server = root / "volna-server"
            server.write_bytes(binary("linux-x64"))
            vsix = root / "volna.vsix"

            def write_vsix(target, bundled):
                identity = f' TargetPlatform="{target}"' if target else ""
                manifest = f'<PackageManifest xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011"><Metadata><Identity Version="0.1.0"{identity}/></Metadata></PackageManifest>'
                with zipfile.ZipFile(vsix, "w") as archive:
                    archive.writestr("extension.vsixmanifest", manifest)
                    archive.writestr("extension/package.json", json.dumps({"version": "0.1.0"}))
                    archive.writestr("extension/bin/volna-server", bundled)
                    for item in packager.MEDIA:
                        archive.writestr(f"extension/media/{item}", b"content")

            write_vsix(None, server.read_bytes())
            with self.assertRaisesRegex(ValueError, "targetPlatform"):
                packager.verify_vsix(vsix, "linux-x64", server)
            write_vsix("linux-x64", binary("linux-arm64"))
            with self.assertRaisesRegex(ValueError, "does not match"):
                packager.verify_vsix(vsix, "linux-x64", server)
            write_vsix("linux-x64", server.read_bytes())
            packager.verify_vsix(vsix, "linux-x64", server)


if __name__ == "__main__":
    unittest.main()
