"""Stage a desktop distribution from a built Volna binary; never launch it."""

import argparse
import importlib.util
from pathlib import Path
import plistlib
import shutil
import tomllib

FRONTEND = Path(__file__).resolve().parents[1]
ASSETS = FRONTEND / "assets" / "app-icon"
spec = importlib.util.spec_from_file_location("distribution_notices", Path(__file__).with_name("distribution-notices.py"))
notices = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notices)
APP_ID = "io.github.ripopov.volna"


def copy(source, destination, executable=False):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(0o755 if executable else 0o644)


def package(platform, binary, output):
    if not binary.is_file():
        raise FileNotFoundError(f"Build Volna first: {binary}")
    if platform not in ("macos", "linux", "windows"):
        raise ValueError(f"Unsupported platform: {platform}")
    notice_dir = (output / "Volna.app/Contents/Resources/licenses" if platform == "macos"
                  else output / "share/doc/volna/licenses" if platform == "linux"
                  else output / "licenses")
    notices.collect(notice_dir, {"volna"})
    if platform == "macos":
        contents = output / "Volna.app" / "Contents"
        copy(binary, contents / "MacOS" / "volna", executable=True)
        copy(ASSETS / "volna.icns", contents / "Resources" / "volna.icns")
        manifest = tomllib.loads((FRONTEND.parents[1] / "Cargo.toml").read_text())
        version = manifest["workspace"]["package"]["version"]
        metadata = {
            "CFBundleDevelopmentRegion": "en",
            "CFBundleExecutable": "volna",
            "CFBundleIconFile": "volna.icns",
            "CFBundleIdentifier": APP_ID,
            "CFBundleName": "Volna",
            "CFBundleDisplayName": "Volna",
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": version,
            "CFBundleVersion": version,
            "NSHighResolutionCapable": True,
            "NSPrincipalClass": "NSApplication",
        }
        (contents / "Info.plist").write_bytes(plistlib.dumps(metadata))
        (contents / "PkgInfo").write_bytes(b"APPL????")
        return contents.parent
    if platform == "linux":
        copy(binary, output / "bin" / "volna", executable=True)
        share = output / "share"
        copy(
            FRONTEND / "packaging" / f"{APP_ID}.desktop",
            share / "applications" / f"{APP_ID}.desktop",
        )
        icons = share / "icons" / "hicolor"
        copy(ASSETS / "volna.svg", icons / "scalable" / "apps" / f"{APP_ID}.svg")
        for size in (16, 24, 32, 48, 64, 128, 256, 512, 1024):
            copy(
                ASSETS / f"volna-{size}.png",
                icons / f"{size}x{size}" / "apps" / f"{APP_ID}.png",
            )
        return output
    if platform == "windows":
        copy(binary, output / "volna.exe", executable=True)
        return output / "volna.exe"
    raise ValueError(f"Unsupported platform: {platform}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=("macos", "linux", "windows"), required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    print(package(args.platform, args.binary.resolve(), args.output.resolve()))
