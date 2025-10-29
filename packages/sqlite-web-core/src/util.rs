use js_sys::Reflect;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;

pub fn sanitize_identifier(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => c,
            _ => '_',
        })
        .collect();
    if s.is_empty() {
        "db".to_string()
    } else {
        s
    }
}

pub fn sanitize_db_filename(name: &str) -> String {
    let mut id = sanitize_identifier(name);
    if !id.ends_with(".db") {
        id.push_str(".db");
    }
    id
}

pub fn set_js_property(target: &JsValue, key: &str, value: &JsValue) -> Result<(), JsValue> {
    match Reflect::set(target, &JsValue::from_str(key), value) {
        Ok(true) => Ok(()),
        Ok(false) => Err(JsValue::from_str(&format!(
            "Reflect::set returned false for key {key}"
        ))),
        Err(err) => Err(err),
    }
}

pub fn js_value_to_string(value: &JsValue) -> String {
    if let Some(s) = value.as_string() {
        return s;
    }
    // Handle BigInt explicitly before attempting JSON.stringify, which would throw.
    if let Some(bi) = value.dyn_ref::<js_sys::BigInt>() {
        if let Ok(js_s) = bi.to_string(10) {
            let s: String = js_s.into();
            return s;
        }
    }
    if let Some(error) = value.dyn_ref::<js_sys::Error>() {
        return error
            .to_string()
            .as_string()
            .unwrap_or_else(|| format!("{value:?}"));
    }
    match js_sys::JSON::stringify(value) {
        Ok(s) => s.as_string().unwrap_or_else(|| format!("{value:?}")),
        Err(_) => format!("{value:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_identifier_basic() {
        assert_eq!(sanitize_identifier("test"), "test");
        assert_eq!(sanitize_identifier(" test  "), "test");
        assert_eq!(sanitize_identifier("weird name!*"), "weird_name__");
        assert_eq!(sanitize_identifier(""), "db");
    }

    #[test]
    fn test_sanitize_db_filename() {
        assert_eq!(sanitize_db_filename("mydb"), "mydb.db");
        assert_eq!(sanitize_db_filename("mydb.db"), "mydb.db");
        assert_eq!(sanitize_db_filename("bad/name"), "bad_name.db");
        assert_eq!(sanitize_db_filename(""), "db.db");
    }
}

#[cfg(all(test, target_family = "wasm"))]
mod wasm_tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn js_value_to_string_handles_error() {
        let err = js_sys::Error::new("something went wrong");
        let v: JsValue = err.into();
        let s = js_value_to_string(&v);
        assert!(s.contains("something went wrong"));
    }

    #[wasm_bindgen_test]
    fn js_value_to_string_handles_bigint() {
        let bi = js_sys::BigInt::from(1234u32);
        let v: JsValue = bi.into();
        let s = js_value_to_string(&v);
        assert_eq!(s, "1234");
    }

    #[wasm_bindgen_test]
    fn js_value_to_string_handles_plain_object() {
        let obj = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("a"), &JsValue::from_f64(1.0));
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("b"), &JsValue::from_str("x"));
        let v: JsValue = obj.into();
        let s = js_value_to_string(&v);
        // Expect JSON string representation
        assert!(s.starts_with('{') && s.ends_with('}'));
        assert!(s.contains("\"a\":1"));
        assert!(s.contains("\"b\":\"x\""));
    }
}
