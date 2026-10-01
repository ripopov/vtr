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
    @classmethod
    def setUpClass(cls):
        cls.notices_temp = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.notices_temp.cleanup)
        cls.notices_dir = Path(cls.notices_temp.name)
        packager.notices.collect(cls.notices_dir, {"volna", "volna-server"})

    def add_notices(self, archive):
        for file in self.notices_dir.rglob("*"):
            if file.is_file():
                archive.write(file, "extension/licenses/" + file.relative_to(self.notices_dir).as_posix())

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

            def write_vsix(target, bundled, notices=True):
                identity = f' TargetPlatform="{target}"' if target else ""
                manifest = f'<PackageManifest xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011"><Metadata><Identity Version="0.1.0"{identity}/></Metadata></PackageManifest>'
                with zipfile.ZipFile(vsix, "w") as archive:
                    if notices:
                        self.add_notices(archive)
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
            write_vsix("linux-x64", server.read_bytes(), notices=False)
            with self.assertRaises(KeyError):
                packager.verify_vsix(vsix, "linux-x64", server)

    def test_notices_reject_missing_files(self):
        files = {p.relative_to(self.notices_dir).as_posix(): p.read_bytes()
                 for p in self.notices_dir.rglob("*") if p.is_file()}
        for name in (*packager.notices.STATIC, "DEPENDENCIES.json", "FILES.json",
                     next(n for n in files if n.startswith("dependencies/"))):
            with self.subTest(name=name):
                missing = dict(files)
                del missing[name]
                with self.assertRaises(KeyError):
                    packager.notices.verify(missing.__getitem__)

    def test_vsix_rejects_missing_snippet_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            server = root / "volna-server"
            server.write_bytes(binary("linux-x64"))
            vsix = root / "volna.vsix"
            with zipfile.ZipFile(vsix, "w") as archive:
                self.add_notices(archive)
                archive.writestr("extension.vsixmanifest", '<PackageManifest xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011"><Metadata><Identity Version="0.1.1" TargetPlatform="linux-x64"/></Metadata></PackageManifest>')
                archive.writestr("extension/package.json", json.dumps({"version": "0.1.1"}))
                archive.writestr("extension/bin/volna-server", server.read_bytes())
                archive.writestr("extension/media/theme.mjs", b"export const theme = {};")
                archive.writestr("extension/media/volna_bg.wasm", b"wasm")
                archive.writestr("extension/media/volna.js", "import { copyTableText } from './snippets/generated/src/table_clipboard.mjs';")
            with self.assertRaisesRegex(ValueError, "missing media/snippets/generated/src/table_clipboard.mjs"):
                packager.verify_vsix(vsix, "linux-x64", server)
            with zipfile.ZipFile(vsix, "a") as archive:
                archive.writestr("extension/media/snippets/generated/src/table_clipboard.mjs", "export { copyTableText } from '../copy.mjs';")
            with self.assertRaisesRegex(ValueError, "missing media/snippets/generated/copy.mjs"):
                packager.verify_vsix(vsix, "linux-x64", server)
            with zipfile.ZipFile(vsix, "a") as archive:
                archive.writestr("extension/media/snippets/generated/copy.mjs", "export function copyTableText() {}")
            packager.verify_vsix(vsix, "linux-x64", server)


if __name__ == "__main__":
    unittest.main()
