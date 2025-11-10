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
