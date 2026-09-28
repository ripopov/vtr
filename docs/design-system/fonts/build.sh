#!/usr/bin/env bash
# Regenerates the site's WOFF2 fonts from the viewer's TTFs in
# volna/volna-core/assets/fonts (the originals and their licences).
# IBM Plex Sans is converted whole: its OFL reserves the name "Plex", and a
# subset would be a modified version. Lilex has no reserved name, so it is
# subset to Latin-1 plus the punctuation, arrows and symbols the site uses.
# Needs fonttools and brotli (pip install fonttools brotli).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
src="$here/../../../volna/volna-core/assets/fonts"
for f in IBMPlexSans-Regular IBMPlexSans-SemiBold; do
  python3 -c "import sys; from fontTools.ttLib import TTFont; t = TTFont(sys.argv[1], recalcTimestamp=False); t.flavor = 'woff2'; t.save(sys.argv[2])" \
    "$src/$f.ttf" "$here/$f.woff2"
done
pyftsubset "$src/Lilex-Regular.ttf" --flavor=woff2 --layout-features='*' --output-file="$here/Lilex-Regular.woff2" \
  --unicodes='U+0000-00FF,U+0131,U+0152-0153,U+02C6,U+02DA,U+02DC,U+2000-206F,U+20AC,U+2122,U+2190-21FF,U+2212,U+2215,U+221E,U+2248,U+2260,U+2264-2265,U+2300-23FF,U+25A0-25FF,U+2713,U+2715,U+FFFD'
