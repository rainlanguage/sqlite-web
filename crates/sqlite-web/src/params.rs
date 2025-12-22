use base64::Engine;
use js_sys::{Array, ArrayBuffer, BigInt, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

use crate::errors::SQLiteWasmDatabaseError;

pub(crate) fn normalize_params_js(params: &JsValue) -> Result<Array, SQLiteWasmDatabaseError> {
    let arr = ensure_array(params)?;
    (0..arr.length()).try_fold(Array::new(), |normalized, i| {
        let nv = normalize_one_param(&arr.get(i), i)?;
        normalized.push(&nv);
        Ok(normalized)
    })
}

fn ensure_array(params: &JsValue) -> Result<Array, SQLiteWasmDatabaseError> {
    if params.is_undefined() || params.is_null() {
        return Ok(Array::new());
    }
    if Array::is_array(params) {
        return Ok(params.clone().unchecked_into());
    }
    Err(SQLiteWasmDatabaseError::JsError(JsValue::from_str(
        "params must be an array",
    )))
}

fn normalize_one_param(v: &JsValue, index: u32) -> Result<JsValue, SQLiteWasmDatabaseError> {
    if v.is_null() || v.is_undefined() {
        return Ok(JsValue::NULL);
    }
    if let Ok(bi) = v.clone().dyn_into::<BigInt>() {
        return encode_bigint_to_obj(bi);
    }
    if let Ok(typed) = v.clone().dyn_into::<Uint8Array>() {
        return encode_binary_to_obj(typed.to_vec());
    }
    if let Ok(buf) = v.clone().dyn_into::<ArrayBuffer>() {
        let typed = Uint8Array::new(&buf);
        return encode_binary_to_obj(typed.to_vec());
    }
    if let Some(n) = v.as_f64() {
        if !n.is_finite() {
            return Err(SQLiteWasmDatabaseError::JsError(JsValue::from_str(
                &format!(
                    "Invalid numeric value at position {} (NaN/Infinity not supported.)",
                    index + 1
                ),
            )));
        }
        return Ok(JsValue::from_f64(n));
    }
    if let Some(b) = v.as_bool() {
        return Ok(JsValue::from_bool(b));
    }
    if let Some(s) = v.as_string() {
        return Ok(JsValue::from_str(&s));
    }
    Err(SQLiteWasmDatabaseError::JsError(JsValue::from_str(
        &format!("Unsupported parameter type at position {}", index + 1),
    )))
}

fn encode_bigint_to_obj(bi: BigInt) -> Result<JsValue, SQLiteWasmDatabaseError> {
    let obj = Object::new();
    let s = bi
        .to_string(10)
        .map_err(|e| SQLiteWasmDatabaseError::JsError(e.into()))?;
    Reflect::set(
        &obj,
        &JsValue::from_str("__type"),
        &JsValue::from_str("bigint"),
    )
    .map_err(SQLiteWasmDatabaseError::from)?;
    Reflect::set(&obj, &JsValue::from_str("value"), &JsValue::from(s))
        .map_err(SQLiteWasmDatabaseError::from)?;
    Ok(obj.into())
}

fn encode_binary_to_obj(bytes: Vec<u8>) -> Result<JsValue, SQLiteWasmDatabaseError> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let obj = Object::new();
    Reflect::set(
        &obj,
        &JsValue::from_str("__type"),
        &JsValue::from_str("blob"),
    )
    .map_err(SQLiteWasmDatabaseError::from)?;
    Reflect::set(&obj, &JsValue::from_str("base64"), &JsValue::from_str(&b64))
        .map_err(SQLiteWasmDatabaseError::from)?;
    Ok(obj.into())
}

#[cfg(all(test, target_family = "wasm"))]
mod tests {
    use super::*;
    use base64::Engine;
    use js_sys::{Array, ArrayBuffer, BigInt, Uint8Array};
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn ensure_array_accepts_null_like_values() {
        let result = ensure_array(&JsValue::NULL).expect("null coerces to empty array");
        assert_eq!(result.length(), 0);

        let undef = ensure_array(&JsValue::UNDEFINED).expect("undefined coerces to empty array");
        assert_eq!(undef.length(), 0);
    }

    #[wasm_bindgen_test]
    fn ensure_array_rejects_non_arrays() {
        let not_array = JsValue::from_str("nope");
        let err = ensure_array(&not_array).expect_err("strings should be rejected");
        match err {
            SQLiteWasmDatabaseError::JsError(js) => {
                assert_eq!(js.as_string().as_deref(), Some("params must be an array"))
            }
            _ => panic!("expected JsError"),
        }
    }

    #[wasm_bindgen_test]
    fn normalize_one_param_rejects_non_finite_numbers() {
        assert!(normalize_one_param(&JsValue::from_f64(f64::NAN), 0).is_err());
        assert!(normalize_one_param(&JsValue::from_f64(f64::INFINITY), 0).is_err());
    }

    #[wasm_bindgen_test]
    fn encode_bigint_marks_type() {
        let encoded = encode_bigint_to_obj(BigInt::from(5u8)).expect("encodes bigint");
        let ty = Reflect::get(&encoded, &JsValue::from_str("__type"))
            .unwrap()
            .as_string();
        assert_eq!(ty.as_deref(), Some("bigint"));
        let val = Reflect::get(&encoded, &JsValue::from_str("value"))
            .unwrap()
            .as_string();
        assert_eq!(val.as_deref(), Some("5"));
    }

    #[wasm_bindgen_test]
    fn encode_binary_emits_base64() {
        let encoded =
            encode_binary_to_obj(vec![1u8, 2, 3, 4]).expect("binary encoding should succeed");
        let ty = Reflect::get(&encoded, &JsValue::from_str("__type"))
            .unwrap()
            .as_string();
        assert_eq!(ty.as_deref(), Some("blob"));

        let base64_val = Reflect::get(&encoded, &JsValue::from_str("base64"))
            .unwrap()
            .as_string()
            .unwrap();
        let expected = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3, 4]);
        assert_eq!(base64_val, expected);
    }

    #[wasm_bindgen_test]
    fn normalize_params_js_handles_arrays() {
        let arr = Array::new();
        arr.push(&JsValue::from_f64(1.0));
        arr.push(&JsValue::from_str("abc"));
        let buf = ArrayBuffer::new(2);
        Uint8Array::new(&buf).copy_from(&[9u8, 8]);
        arr.push(&JsValue::from(buf));

        let normalized = normalize_params_js(&JsValue::from(arr)).expect("valid params");
        assert_eq!(normalized.length(), 3);
        assert_eq!(normalized.get(0).as_f64(), Some(1.0));
        assert_eq!(normalized.get(1).as_string().as_deref(), Some("abc"));
        let blob = normalized.get(2);
        let b64 = Reflect::get(&blob, &JsValue::from_str("base64"))
            .unwrap()
            .as_string()
            .unwrap();
        let expected = base64::engine::general_purpose::STANDARD.encode([9u8, 8]);
        assert_eq!(b64, expected);
    }
}
