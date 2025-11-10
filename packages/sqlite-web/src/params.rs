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
