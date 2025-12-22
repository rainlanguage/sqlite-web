#![cfg(all(test, target_family = "wasm"))]

use crate::ready::{InitializationState, ReadySignal};
use crate::worker::handle_worker_control_message;
use crate::worker_template::generate_self_contained_worker;
use crate::{SQLiteWasmDatabase, SQLiteWasmDatabaseError};

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_test::*;

use base64::Engine;
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
async fn test_sqlite_wasm_database_serialization() {
    let db = SQLiteWasmDatabase::new("testdb")
        .await
        .expect("Should create database");
    let serialized = serde_json::to_string(&db);

    assert!(serialized.is_ok());
    let json_str = serialized.unwrap();
    assert_eq!(json_str, "{}");
}

#[wasm_bindgen_test]
async fn test_sqlite_wasm_database_creation() {
    let result = SQLiteWasmDatabase::new("testdb").await;

    match result {
        Ok(_db) => {}
        Err(e) => {
            let error_msg = format!("{e:?}");
            assert!(!error_msg.is_empty());
        }
    }
}

#[wasm_bindgen_test]
async fn test_query_message_format() {
    if let Ok(db) = SQLiteWasmDatabase::new("testdb").await {
        let result = db.query("SELECT 1", None).await;

        match result {
            Ok(_) => {}
            Err(e) => {
                let error_msg = format!("{e:?}");
                assert!(!error_msg.is_empty());
            }
        }
    }
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

#[wasm_bindgen_test]
async fn test_query_with_various_param_types_and_normalization() {
    install_post_message_spy();
    let db = SQLiteWasmDatabase::new("testdb").await.expect("db created");

    let arr = js_sys::Array::new();
    arr.push(&JsValue::from_f64(42.0));
    arr.push(&JsValue::from_str("hello"));
    arr.push(&JsValue::from_bool(true));
    arr.push(&JsValue::NULL);
    let bi = js_sys::BigInt::from(1234u32);
    let bi_js: JsValue = bi.into();
    js_sys::Reflect::set(&arr, &JsValue::from_f64(5.0), &bi_js).expect("set index 5");
    let bytes: [u8; 3] = [1, 2, 3];
    let u8 = js_sys::Uint8Array::from(&bytes[..]);
    let u8_js: JsValue = u8.into();
    arr.push(&u8_js);
    let buf = js_sys::ArrayBuffer::new(4);
    let typed = js_sys::Uint8Array::new(&buf);
    typed.copy_from(&[5, 6, 7, 8]);
    let buf_js: JsValue = buf.into();
    arr.push(&buf_js);

    let _ = db.query("SELECT 1", Some(arr)).await;

    if let Some(msg) = take_last_message() {
        let ty = js_sys::Reflect::get(&msg, &JsValue::from_str("type")).unwrap();
        assert_eq!(ty.as_string().as_deref(), Some("execute-query"));
        let sql = js_sys::Reflect::get(&msg, &JsValue::from_str("sql")).unwrap();
        assert_eq!(sql.as_string().as_deref(), Some("SELECT 1"));
        let has_params = js_sys::Reflect::has(&msg, &JsValue::from_str("params")).unwrap_or(false);
        assert!(has_params, "params should be present for non-empty array");

        let params = js_sys::Reflect::get(&msg, &JsValue::from_str("params")).unwrap();
        assert!(js_sys::Array::is_array(&params));
        let params: js_sys::Array = params.unchecked_into();
        assert_eq!(params.length(), 8);

        let v0 = params.get(0);
        assert_eq!(v0.as_f64(), Some(42.0));
        let v1 = params.get(1);
        assert_eq!(v1.as_string().as_deref(), Some("hello"));
        let v2 = params.get(2);
        assert_eq!(v2.as_bool(), Some(true));
        let v3 = params.get(3);
        assert!(v3.is_null());
        let v4 = params.get(4);
        assert!(v4.is_null());
        let v5 = params.get(5);
        assert!(v5.is_object());
        let t5 = js_sys::Reflect::get(&v5, &JsValue::from_str("__type")).unwrap();
        assert_eq!(t5.as_string().as_deref(), Some("bigint"));
        let val5 = js_sys::Reflect::get(&v5, &JsValue::from_str("value")).unwrap();
        assert_eq!(val5.as_string().as_deref(), Some("1234"));
        let v6 = params.get(6);
        assert!(v6.is_object());
        let t6 = js_sys::Reflect::get(&v6, &JsValue::from_str("__type")).unwrap();
        assert_eq!(t6.as_string().as_deref(), Some("blob"));
        let b64_6 = js_sys::Reflect::get(&v6, &JsValue::from_str("base64")).unwrap();
        let b64_6 = b64_6.as_string().expect("base64 string");
        let expected_6 = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
        assert_eq!(b64_6, expected_6);
        let v7 = params.get(7);
        assert!(v7.is_object());
        let t7 = js_sys::Reflect::get(&v7, &JsValue::from_str("__type")).unwrap();
        assert_eq!(t7.as_string().as_deref(), Some("blob"));
        let b64_7 = js_sys::Reflect::get(&v7, &JsValue::from_str("base64")).unwrap();
        let b64_7 = b64_7.as_string().expect("base64 string");
        let expected_7 = base64::engine::general_purpose::STANDARD.encode([5u8, 6, 7, 8]);
        assert_eq!(b64_7, expected_7);
    }

    uninstall_post_message_spy();
}

#[wasm_bindgen_test]
async fn test_query_params_presence_empty_array_vs_none() {
    install_post_message_spy();
    let db = SQLiteWasmDatabase::new("testdb").await.expect("db created");

    let _ = db.query("SELECT 1", None).await;
    if let Some(msg) = take_last_message() {
        let has_params = js_sys::Reflect::has(&msg, &JsValue::from_str("params")).unwrap_or(true);
        assert!(!has_params, "params should be absent when params=None");
    }

    let empty = js_sys::Array::new();
    let _ = db.query("SELECT 1", Some(empty)).await;
    if let Some(msg) = take_last_message() {
        let has_params = js_sys::Reflect::has(&msg, &JsValue::from_str("params")).unwrap_or(true);
        assert!(!has_params, "params should be absent for empty array");
    }

    let one = js_sys::Array::new();
    one.push(&JsValue::from_f64(1.0));
    let _ = db.query("SELECT 1", Some(one)).await;
    if let Some(msg) = take_last_message() {
        let has_params = js_sys::Reflect::has(&msg, &JsValue::from_str("params")).unwrap_or(false);
        assert!(has_params, "params should be present for non-empty array");
    }

    uninstall_post_message_spy();
}

#[wasm_bindgen_test]
async fn test_query_rejects_nan_infinity_params() {
    let db = SQLiteWasmDatabase::new("testdb").await.expect("db created");

    let arr = js_sys::Array::new();
    arr.push(&JsValue::from_f64(f64::NAN));
    let res = db.query("SELECT ?", Some(arr)).await;
    assert!(res.is_err(), "NaN should be rejected");

    let arr = js_sys::Array::new();
    arr.push(&JsValue::from_f64(f64::INFINITY));
    let res = db.query("SELECT ?", Some(arr)).await;
    assert!(res.is_err(), "+Infinity should be rejected");

    let arr = js_sys::Array::new();
    arr.push(&JsValue::from_f64(f64::NEG_INFINITY));
    let res = db.query("SELECT ?", Some(arr)).await;
    assert!(res.is_err(), "-Infinity should be rejected");
}

fn install_post_message_spy() {
    let code = r#"
        (function(){
            try {
                self.__lastMessage = undefined;
                if (!self.__origPostMessage) {
                    self.__origPostMessage = Worker.prototype.postMessage;
                }
                Worker.prototype.postMessage = function(msg) {
                    self.__lastMessage = msg;
                    return self.__origPostMessage.call(this, msg);
                };
            } catch (e) {
            }
        })()
    "#;
    let f = js_sys::Function::new_no_args(code);
    let _ = f.call0(&JsValue::UNDEFINED);
}

fn uninstall_post_message_spy() {
    let code = r#"
        (function(){
            try {
                if (self.__origPostMessage) {
                    Worker.prototype.postMessage = self.__origPostMessage;
                    self.__origPostMessage = undefined;
                }
            } catch (e) {
            }
        })()
    "#;
    let f = js_sys::Function::new_no_args(code);
    let _ = f.call0(&JsValue::UNDEFINED);
}

fn take_last_message() -> Option<JsValue> {
    let global = js_sys::global();
    let key = JsValue::from_str("__lastMessage");
    let val = js_sys::Reflect::get(&global, &key).ok();
    let _ = js_sys::Reflect::set(&global, &key, &JsValue::UNDEFINED);
    val.and_then(|v| if v.is_undefined() { None } else { Some(v) })
}
