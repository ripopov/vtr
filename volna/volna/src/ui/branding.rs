//! Theme-coloured application marks, distinct from trace semantic icons.

use gpui_kit::{Hsla, Pixels, Styled, Svg, svg};

pub(crate) const SEAL_PATH: &str = "branding/volna-mark.svg";
pub(crate) const ASSETS: [(&str, &[u8]); 1] = [(
    SEAL_PATH,
    include_bytes!("../../assets/app-icon/volna-mark.svg"),
)];

pub(crate) fn seal(size: Pixels, color: Hsla) -> Svg {
    svg()
        .path(SEAL_PATH)
        .size(size)
        .flex_none()
        .text_color(color)
}
