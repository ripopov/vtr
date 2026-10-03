# Volna application icon

`volna.svg` is the canonical, first-party vector artwork.
It uses the selected Soft Tile design: taller V-shaped blue waves, a slightly
larger pearl-white seal, and a softly shaded navy tile with transparent margins.
All PNG, ICO and ICNS files in this directory are
generated from it; do not edit those files independently.

`volna-mark.svg` is a simplified, single-colour seal-and-wave variant used in
the welcome screen and trace start panel. This hand-drawn vector mark inherits
the current theme colours and is embedded by the GPUI asset source on both
native and web builds; it is not a generated desktop icon.
Its seal and wave proportions match the canonical artwork.

Docs logos and favicons reference this directory directly, so regenerating the
canonical assets updates the documentation branding without copying icons.

From the repository root, with [uv](https://docs.astral.sh/uv/) installed:

```sh
uv run --script volna/volna/tools/generate-icons.py
uv run --script volna/volna/tools/check-desktop.py
```

The scripts pin resvg-py and Pillow and require Python 3.11 or newer. `uv`
provisions their isolated dependencies. The check also uses pefile to inspect
Windows executable resources. Linux checks require `desktop-file-utils`.
Every raster size is rendered directly from the SVG. The ICO contains
16, 24, 32, 48, 64, 128 and 256 px representations; the ICNS includes standard
and Retina representations through 1024 px. PNGs are also installed into
Linux's hicolor icon theme, alongside the scalable SVG.

See [desktop packaging](../../README.md#desktop-packaging-and-application-icons)
for build and installation commands.
