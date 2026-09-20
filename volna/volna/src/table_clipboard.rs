//! Browser clipboard recovery. The complete, already bounded TSV is passed
//! synchronously from the input handler so the host can preserve the gesture.

use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/table_clipboard.mjs")]
extern "C" {
    #[wasm_bindgen(js_name = copyTableText)]
    fn copy_table_text(text: &str);
}

pub(crate) fn copy(text: &str) {
    copy_table_text(text);
}
