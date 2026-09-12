//! JS wire decoding only; theme resolution consumes a typed palette.
use super::{Appearance, ColorPair, HostPalette};
use wasm_bindgen::JsValue;

pub(crate) fn decode(appearance: Appearance, value: &JsValue) -> HostPalette {
    let get = |object: &JsValue, key: &str| {
        js_sys::Reflect::get(object, &key.into()).unwrap_or(JsValue::UNDEFINED)
    };
    let color = |object: &JsValue, key: &str| {
        let n = get(object, key).as_f64()?;
        (n.is_finite() && n.fract() == 0.0 && (0.0..=u32::MAX as f64).contains(&n))
            .then(|| gpui::rgba(n as u32).into())
    };
    let pair = |key| {
        let pair = get(value, key);
        ColorPair {
            background: color(&pair, "background"),
            foreground: color(&pair, "foreground"),
        }
    };
    HostPalette {
        appearance,
        editor: pair("editor"),
        panel: pair("panel"),
        bar: pair("bar"),
        elevated: pair("elevated"),
        tooltip: pair("tooltip"),
        input: pair("input"),
        selection: pair("selection"),
        hover: pair("hover"),
        button: pair("button"),
        menu_hover: pair("menu_hover"),
        button_hover: color(value, "button_hover"),
        border: color(value, "border"),
        panel_border: color(value, "panel_border"),
        input_border: color(value, "input_border"),
        focus: color(value, "focus"),
        contrast_border: color(value, "contrast_border"),
        muted: color(value, "muted"),
        placeholder: color(value, "placeholder"),
        icon: color(value, "icon"),
        accent: color(value, "accent"),
        error: color(value, "error"),
        scrollbar: color(value, "scrollbar"),
        scrollbar_hover: color(value, "scrollbar_hover"),
        charts: ["green", "red", "yellow", "blue", "orange", "purple"]
            .map(|name| color(&get(value, "charts"), name)),
        cursor: color(value, "cursor"),
    }
}
