# /// script
# requires-python = ">=3.11"
# dependencies = ["resvg-py==0.5.0", "Pillow==12.1.0", "pefile==2024.8.26"]
# ///
"""Headless icon and packaging checks, with optional built-binary validation."""

import argparse
import configparser
import importlib.util
import io
import os
from pathlib import Path
import plistlib
import struct
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
import zipfile

from PIL import Image
import pefile


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(f"{name}.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


generate = load("generate-icons")
package = load("package-desktop")
ASSETS = generate.ASSETS
FRONTEND = package.FRONTEND


def rgba(data):
    return Image.open(io.BytesIO(data)).convert("RGBA")


class DesktopTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.pngs = generate.render_icons()

    def test_pngs_match_canonical_svg(self):
        root = ET.parse(ASSETS / "volna.svg").getroot()
        self.assertEqual(root.attrib["viewBox"], "0 0 1254 1254")
        self.assertFalse(root.findall(".//{http://www.w3.org/2000/svg}image"))
        for size, expected in self.pngs.items():
            with self.subTest(size=size):
                actual = rgba((ASSETS / f"volna-{size}.png").read_bytes())
                self.assertEqual(actual.size, (size, size))
                self.assertEqual(actual.tobytes(), rgba(expected).tobytes())
                self.assertGreater(len(actual.getcolors(size * size)), 100)
                for corner in ((0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)):
                    self.assertEqual(actual.getpixel(corner)[3], 0)
                self.assertEqual(actual.getpixel((size // 2, size // 2))[3], 255)

    def test_ico_representations(self):
        with Image.open(ASSETS / "volna.ico") as ico:
            self.assertEqual(ico.ico.sizes(), {(size, size) for size in generate.ICO_SIZES})
            for size in generate.ICO_SIZES:
                actual = ico.ico.getimage((size, size)).convert("RGBA")
                self.assertEqual(actual.tobytes(), rgba(self.pngs[size]).tobytes())

    def test_icns_representations(self):
        data = (ASSETS / "volna.icns").read_bytes()
        self.assertEqual(data[:4], b"icns")
        self.assertEqual(struct.unpack_from(">I", data, 4)[0], len(data))
        offset, found = 8, {}
        while offset < len(data):
            tag, length = struct.unpack_from(">4sI", data, offset)
            self.assertGreater(length, 8)
            self.assertNotIn(tag, found)
            size = generate.ICNS_SIZES[tag]
            actual = rgba(data[offset + 8:offset + length])
            self.assertEqual(actual.size, (size, size))
            self.assertEqual(actual.tobytes(), rgba(self.pngs[size]).tobytes())
            found[tag] = size
            offset += length
        self.assertEqual(offset, len(data))
        self.assertEqual(found, generate.ICNS_SIZES)
        # An independent decoder must also recognize the ICNS container.
        with Image.open(ASSETS / "volna.icns") as icon:
            icon.load()
            self.assertEqual(icon.size, (1024, 1024))

    def test_distribution_layouts(self):
        with tempfile.TemporaryDirectory(prefix="volna desktop ") as directory:
            temp = Path(directory)
            binary = temp / "input binary"
            binary.write_bytes(b"isolated executable fixture")
            for platform in ("macos", "linux", "windows"):
                output = temp / platform
                result = package.package(platform, binary, output)
                self.assertTrue(result.exists())
                # Inspect the actual archived layout, including every dependency notice.
                notice_dir = ("Volna.app/Contents/Resources/licenses" if platform == "macos"
                              else "share/doc/volna/licenses" if platform == "linux" else "licenses")
                archive_path = temp / f"{platform}.zip"
                with zipfile.ZipFile(archive_path, "w", zipfile.ZIP_DEFLATED) as archive:
                    for file in output.rglob("*"):
                        if file.is_file():
                            archive.write(file, file.relative_to(output).as_posix())
                with zipfile.ZipFile(archive_path) as archive:
                    package.notices.verify(lambda name: archive.read(f"{notice_dir}/{name}"))
                if platform == "macos":
                    contents = result / "Contents"
                    metadata = plistlib.loads((contents / "Info.plist").read_bytes())
                    self.assertEqual(metadata["CFBundleIdentifier"], package.APP_ID)
                    self.assertEqual(metadata["CFBundlePackageType"], "APPL")
                    executable = contents / "MacOS" / metadata["CFBundleExecutable"]
                    self.assertEqual(executable.read_bytes(), binary.read_bytes())
                    if os.name != "nt":
                        self.assertTrue(executable.stat().st_mode & 0o111)
                    icon = contents / "Resources" / metadata["CFBundleIconFile"]
                    self.assertEqual(icon.read_bytes(), (ASSETS / "volna.icns").read_bytes())
                elif platform == "linux":
                    desktop = output / "share/applications" / f"{package.APP_ID}.desktop"
                    entry = configparser.ConfigParser(interpolation=None)
                    entry.read(desktop)
                    self.assertEqual(entry["Desktop Entry"]["Icon"], package.APP_ID)
                    self.assertEqual(entry["Desktop Entry"]["StartupWMClass"], package.APP_ID)
                    self.assertEqual(entry["Desktop Entry"]["Exec"], "volna %f")
                    self.assertEqual((output / "bin/volna").read_bytes(), binary.read_bytes())
                    for size in generate.SIZES:
                        icon = output / f"share/icons/hicolor/{size}x{size}/apps/{package.APP_ID}.png"
                        self.assertEqual(icon.read_bytes(), (ASSETS / f"volna-{size}.png").read_bytes())
                    icon = output / f"share/icons/hicolor/scalable/apps/{package.APP_ID}.svg"
                    self.assertEqual(icon.read_bytes(), (ASSETS / "volna.svg").read_bytes())
                    if sys.platform.startswith("linux"):
                        subprocess.run(["desktop-file-validate", str(desktop)], check=True, timeout=15)
                else:
                    self.assertEqual(result.read_bytes(), binary.read_bytes())


def check_binary(binary, platform):
    """Validate the actual linked icon, not merely the resource script."""
    help_result = subprocess.run(
        [str(binary), "--help"], capture_output=True, text=True, check=True, timeout=30
    )
    assert "usage: volna" in help_result.stdout
    data = binary.read_bytes()
    if platform == "windows":
        with pefile.PE(str(binary)) as pe:
            resources = {entry.id: entry for entry in pe.DIRECTORY_ENTRY_RESOURCE.entries}
            groups = {entry.id: entry for entry in resources[14].directory.entries}
            group = groups[1].directory.entries[0].data.struct
            payload = pe.get_data(group.OffsetToData, group.Size)
            reserved, kind, count = struct.unpack_from("<HHH", payload)
            assert (reserved, kind, count) == (0, 1, len(generate.ICO_SIZES))
            icons = {entry.id: entry for entry in resources[3].directory.entries}
            sizes = set()
            with Image.open(ASSETS / "volna.ico") as expected:
                for index in range(count):
                    width, height, _, _, _, _, length, icon_id = struct.unpack_from(
                        "<BBBBHHIH", payload, 6 + 14 * index
                    )
                    size = (width or 256, height or 256)
                    sizes.add(size)
                    resource = icons[icon_id].directory.entries[0].data.struct
                    assert resource.Size == length
                    actual = rgba(pe.get_data(resource.OffsetToData, resource.Size))
                    assert actual.tobytes() == expected.ico.getimage(size).convert("RGBA").tobytes()
            assert sizes == {(size, size) for size in generate.ICO_SIZES}
    else:
        asset = "volna.icns" if platform == "macos" else "volna-256.png"
        assert (ASSETS / asset).read_bytes() in data, f"{binary} is missing its embedded icon"
    with tempfile.TemporaryDirectory(prefix="volna-bundle-") as directory:
        output = Path(directory)
        package.package(platform, binary, output)
        notice_dir = (output / "Volna.app/Contents/Resources/licenses" if platform == "macos"
                      else output / "share/doc/volna/licenses" if platform == "linux" else output / "licenses")
        package.notices.verify(lambda name: (notice_dir / name).read_bytes())
    print(f"Verified linked {platform} application icon: {binary}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--platform", choices=("macos", "linux", "windows"))
    args = parser.parse_args()
    if bool(args.binary) != bool(args.platform):
        parser.error("--binary and --platform must be supplied together")
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(DesktopTests)
    if not unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful():
        sys.exit(1)
    if args.binary:
        check_binary(args.binary.resolve(), args.platform)
