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

#[cfg(all(test, target_family = "wasm"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn describe_handles_strings_and_numbers() {
        assert_eq!(
            describe_js_value(&JsValue::from_str("abc")),
            String::from("abc")
        );
        assert_eq!(describe_js_value(&JsValue::from_f64(42.0)), "42");
        assert_eq!(describe_js_value(&JsValue::from_f64(3.14)), "3.14");
    }

    #[wasm_bindgen_test]
    fn describe_falls_back_to_debug_repr() {
        let obj: JsValue = js_sys::Object::new().into();
        let described = describe_js_value(&obj);
        assert_eq!(
            described,
            format!("{obj:?}"),
            "objects should fall back to Rust debug formatting"
        );
    }
}
