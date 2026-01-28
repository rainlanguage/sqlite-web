#![cfg(all(test, target_family = "wasm"))]

use crate::ready::{InitializationState, ReadySignal};
use crate::worker::handle_worker_control_message;
use crate::worker_template::generate_self_contained_worker;
use crate::SQLiteWasmDatabaseError;

use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

use wasm_bindgen_utils::prelude::{serde_wasm_bindgen, WasmEncodedError};

wasm_bindgen_test_configure!(run_in_browser);

fn make_control_message(msg_type: &str, error: Option<&str>) -> JsValue {
    let msg = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &msg,
        &JsValue::from_str("type"),
        &JsValue::from_str(msg_type),
    );
    if let Some(err) = error {
        let _ = js_sys::Reflect::set(&msg, &JsValue::from_str("error"), &JsValue::from_str(err));
    }
    msg.into()
}

#[wasm_bindgen_test]
fn test_sqlite_wasm_database_error_from_js_value() {
    let js_error = JsValue::from_str("Test error message");
    let db_error = SQLiteWasmDatabaseError::from(js_error);

    match db_error {
        SQLiteWasmDatabaseError::JsError(val) => {
            assert_eq!(val.as_string().unwrap(), "Test error message");
        }
        _ => panic!("Expected JsError variant"),
    }
}

#[wasm_bindgen_test]
fn test_sqlite_wasm_database_error_to_js_value() {
    let db_error = SQLiteWasmDatabaseError::JsError(JsValue::from_str("Test error"));
    let js_value = JsValue::from(db_error);

    assert!(js_value.is_object());
}

#[wasm_bindgen_test]
fn test_sqlite_wasm_database_error_to_wasm_encoded_error() {
    let db_error = SQLiteWasmDatabaseError::JsError(JsValue::from_str("Test error"));
    let wasm_error = WasmEncodedError::from(db_error);

    assert!(wasm_error.msg.contains("Test error"));
    assert!(wasm_error.readable_msg.contains("Test error"));
}

#[wasm_bindgen_test]
fn test_sqlite_wasm_database_error_display() {
    let db_error = SQLiteWasmDatabaseError::JsError(JsValue::from_str("Display test"));
    let error_string = format!("{db_error}");

    assert!(error_string.contains("JavaScript error"));
}

#[wasm_bindgen_test]
fn test_error_propagation_chain() {
    let serde_error = serde_wasm_bindgen::Error::new("Test serde error");
    let db_error = SQLiteWasmDatabaseError::SerdeError(serde_error);

    match db_error {
        SQLiteWasmDatabaseError::SerdeError(_) => {}
        _ => panic!("Expected SerdeError variant"),
    }

    let js_value = JsValue::from(db_error);
    assert!(js_value.is_object());
}

#[wasm_bindgen_test]
fn test_worker_template_generation() {
    let worker_code = generate_self_contained_worker("testdb");

    assert!(!worker_code.is_empty());
    assert!(
        worker_code.contains("importScripts")
            || worker_code.contains("import")
            || worker_code.len() > 100
    );
    assert!(
        worker_code.contains("__SQLITE_FOLLOWER_TIMEOUT_MS"),
        "Worker template should embed follower timeout configuration"
    );
    assert!(
        worker_code.contains("__SQLITE_QUERY_TIMEOUT_MS"),
        "Worker template should embed query timeout configuration"
    );
}

#[wasm_bindgen_test(async)]
async fn test_ready_signal_resolves_on_worker_ready_msg() {
    let signal = ReadySignal::new();
    let promise = signal
        .wait_promise()
        .expect("ready promise should exist before resolution");
    let future = wasm_bindgen_futures::JsFuture::from(promise);

    let message = make_control_message("worker-ready", None);
    let handled = handle_worker_control_message(&message, &signal);

    assert!(handled, "Ready message should be handled");
    assert!(
        matches!(signal.current_state(), InitializationState::Ready),
        "Signal should transition to Ready state"
    );
    let result = future.await;
    assert!(result.is_ok(), "Ready promise should resolve successfully");
}

#[wasm_bindgen_test(async)]
async fn test_ready_signal_rejects_on_worker_error_msg() {
    let signal = ReadySignal::new();
    let promise = signal
        .wait_promise()
        .expect("ready promise should exist before resolution");
    let future = wasm_bindgen_futures::JsFuture::from(promise);

    let message = make_control_message("worker-error", Some("boom"));
    let handled = handle_worker_control_message(&message, &signal);

    assert!(handled, "Error message should be handled");
    match signal.current_state() {
        InitializationState::Failed(reason) => {
            assert_eq!(reason, "boom", "Failure reason should match payload")
        }
        other => panic!("Expected Failed state, got {other:?}"),
    }
    let err = future
        .await
        .expect_err("Promise should reject on worker-error");
    assert_eq!(
        err.as_string().as_deref(),
        Some("boom"),
        "Rejected value should propagate worker error text"
    );
}
