use wasm_bindgen::prelude::*;

pub(crate) fn describe_js_value(value: &JsValue) -> String {
    if let Some(s) = value.as_string() {
        return s;
    }
    if let Some(n) = value.as_f64() {
        if n.fract() == 0.0 {
            return format!("{n:.0}");
        }
        return format!("{n}");
    }
    format!("{value:?}")
}
