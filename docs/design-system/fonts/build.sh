#!/usr/bin/env bash
# Regenerates the site's WOFF2 fonts from the viewer's TTFs in
# volna/volna-core/assets/fonts (the originals and their licences).
# Neither Inter nor JetBrains Mono reserves a font name in its OFL, so both
# are subset to Latin-1 plus Latin Extended-A, Cyrillic and the punctuation,
# arrows and symbols the site uses.
# Needs fonttools and brotli (pip install fonttools brotli).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
src="$here/../../../volna/volna-core/assets/fonts"
unicodes='U+0000-017F,U+0400-045F,U+02C6,U+02DA,U+02DC,U+2000-206F,U+20AC,U+2122,U+2190-21FF,U+2212,U+2215,U+221E,U+2248,U+2260,U+2264-2265,U+2300-23FF,U+25A0-25FF,U+2713,U+2715,U+FFFD'
for f in Inter-Regular Inter-SemiBold JetBrainsMono-Regular; do
  pyftsubset "$src/$f.ttf" --flavor=woff2 --layout-features='*' --output-file="$here/$f.woff2" --unicodes="$unicodes"
done
