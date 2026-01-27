use thiserror::Error;
use wasm_bindgen::prelude::*;
use wasm_bindgen_utils::prelude::{serde_wasm_bindgen, WasmEncodedError};

#[derive(Debug, Error)]
pub enum SQLiteWasmDatabaseError {
    #[error(transparent)]
    SerdeError(#[from] serde_wasm_bindgen::Error),
    #[error("JavaScript error: {0:?}")]
    JsError(JsValue),
    #[error("Initialization pending")]
    InitializationPending,
    #[error("Initialization failed: {0}")]
    InitializationFailed(String),
    #[error("OPFS deletion failed: {0}")]
    OpfsDeletionFailed(String),
}

impl From<JsValue> for SQLiteWasmDatabaseError {
    fn from(value: JsValue) -> Self {
        SQLiteWasmDatabaseError::JsError(value)
    }
}

impl From<SQLiteWasmDatabaseError> for JsValue {
    fn from(value: SQLiteWasmDatabaseError) -> Self {
        JsError::new(&value.to_string()).into()
    }
}

impl From<SQLiteWasmDatabaseError> for WasmEncodedError {
    fn from(value: SQLiteWasmDatabaseError) -> Self {
        WasmEncodedError {
            msg: value.to_string(),
            readable_msg: value.to_string(),
        }
    }
}

#[cfg(all(test, target_family = "wasm"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;
    use wasm_bindgen_utils::prelude::serde_wasm_bindgen;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn js_value_into_error_variant() {
        let js_val = JsValue::from_str("boom");
        match SQLiteWasmDatabaseError::from(js_val) {
            SQLiteWasmDatabaseError::JsError(inner) => {
                assert_eq!(inner.as_string().as_deref(), Some("boom"))
            }
            other => panic!("expected JsError, got {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn error_round_trips_back_into_js_value() {
        let err = SQLiteWasmDatabaseError::InitializationFailed("nope".into());
        let js: JsValue = err.into();
        assert!(js.is_object());
        let error_obj = js_sys::Error::from(js);
        let message = error_obj.message().as_string().unwrap_or_default();
        assert!(
            message.contains("Initialization failed"),
            "message should propagate initialization failure cause"
        );
    }

    #[wasm_bindgen_test]
    fn wasm_encoded_error_keeps_message_human_readable() {
        let err = SQLiteWasmDatabaseError::InitializationPending;
        let wasm_err = WasmEncodedError::from(err);
        assert!(wasm_err.msg.contains("Initialization pending"));
        assert!(wasm_err.readable_msg.contains("Initialization pending"));
    }

    #[wasm_bindgen_test]
    fn serde_error_variant_is_detectable() {
        let serde_err = serde_wasm_bindgen::Error::new("bad serde");
        match SQLiteWasmDatabaseError::SerdeError(serde_err) {
            SQLiteWasmDatabaseError::SerdeError(inner) => {
                assert!(inner.to_string().contains("bad serde"));
            }
            _ => panic!("expected SerdeError variant"),
        }
    }
}
