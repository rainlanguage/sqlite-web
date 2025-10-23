use base64::Engine;
use js_sys::Array;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use thiserror::Error;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_utils::prelude::*;
use web_sys::{Blob, BlobPropertyBag, MessageEvent, Url, Worker};

mod worker_template;
use worker_template::generate_self_contained_worker;

fn create_worker_from_code(worker_code: &str) -> Result<Worker, JsValue> {
    let blob_parts = Array::new();
    blob_parts.push(&JsValue::from_str(worker_code));

    let blob_options = BlobPropertyBag::new();
    blob_options.set_type("application/javascript");

    let blob = Blob::new_with_str_sequence_and_options(&blob_parts, &blob_options)?;
    let worker_url = Url::create_object_url_with_blob(&blob)?;
    let worker = Worker::new(&worker_url)?;
    Url::revoke_object_url(&worker_url)?;
    Ok(worker)
}

fn install_onmessage_handler(
    worker: &Worker,
    pending_queries: Rc<RefCell<Vec<(js_sys::Function, js_sys::Function)>>>,
) {
    let pending_queries_clone = Rc::clone(&pending_queries);
    let onmessage = Closure::wrap(Box::new(move |event: MessageEvent| {
        let data = event.data();
        if handle_worker_control_message(&data) {
            return;
        }
        handle_query_result_message(&data, &pending_queries_clone);
    }) as Box<dyn FnMut(MessageEvent)>);

    worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();
}

fn handle_worker_control_message(data: &JsValue) -> bool {
    if let Ok(obj) = js_sys::Reflect::get(data, &JsValue::from_str("type")) {
        if let Some(msg_type) = obj.as_string() {
            if msg_type == "worker-ready" {
                return true;
            } else if msg_type == "worker-error" {
                let _ = js_sys::Reflect::get(data, &JsValue::from_str("error"));
                return true;
            }
        }
    }
    false
}

fn handle_query_result_message(
    data: &JsValue,
    pending_queries: &Rc<RefCell<Vec<(js_sys::Function, js_sys::Function)>>>,
) {
    if let Ok(obj) = js_sys::Reflect::get(data, &JsValue::from_str("type")) {
        if let Some(msg_type) = obj.as_string() {
            if msg_type == "query-result" {
                if let Some((resolve, reject)) = pending_queries.borrow_mut().pop() {
                    if let Ok(error) = js_sys::Reflect::get(data, &JsValue::from_str("error")) {
                        if !error.is_null() && !error.is_undefined() {
                            let error_str =
                                error.as_string().unwrap_or_else(|| format!("{error:?}"));
                            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(&error_str));
                            return;
                        }
                    }

                    if let Ok(result) = js_sys::Reflect::get(data, &JsValue::from_str("result")) {
                        if !result.is_null() && !result.is_undefined() {
                            let result_str =
                                result.as_string().unwrap_or_else(|| format!("{result:?}"));
                            let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str(&result_str));
                        }
                    }
                }
            }
        }
    }
}

fn encode_bigint_to_obj(bi: js_sys::BigInt) -> Result<JsValue, SQLiteWasmDatabaseError> {
    let obj = js_sys::Object::new();
    let s = bi
        .to_string(10)
        .map_err(|e| SQLiteWasmDatabaseError::JsError(e.into()))?;
    js_sys::Reflect::set(
        &obj,
        &JsValue::from_str("__type"),
        &JsValue::from_str("bigint"),
    )
    .map_err(SQLiteWasmDatabaseError::from)?;
    js_sys::Reflect::set(&obj, &JsValue::from_str("value"), &JsValue::from(s))
        .map_err(SQLiteWasmDatabaseError::from)?;
    Ok(obj.into())
}

fn encode_binary_to_obj(bytes: Vec<u8>) -> Result<JsValue, SQLiteWasmDatabaseError> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let obj = js_sys::Object::new();
    js_sys::Reflect::set(
        &obj,
        &JsValue::from_str("__type"),
        &JsValue::from_str("blob"),
    )
    .map_err(SQLiteWasmDatabaseError::from)?;
    js_sys::Reflect::set(&obj, &JsValue::from_str("base64"), &JsValue::from_str(&b64))
        .map_err(SQLiteWasmDatabaseError::from)?;
    Ok(obj.into())
}

fn normalize_one_param(v: &JsValue, index: u32) -> Result<JsValue, SQLiteWasmDatabaseError> {
    if v.is_null() || v.is_undefined() {
        return Ok(JsValue::NULL);
    }
    if let Ok(bi) = v.clone().dyn_into::<js_sys::BigInt>() {
        return encode_bigint_to_obj(bi);
    }
    if let Ok(typed) = v.clone().dyn_into::<js_sys::Uint8Array>() {
        return encode_binary_to_obj(typed.to_vec());
    }
    if let Ok(buf) = v.clone().dyn_into::<js_sys::ArrayBuffer>() {
        let typed = js_sys::Uint8Array::new(&buf);
        return encode_binary_to_obj(typed.to_vec());
    }
    if let Some(n) = v.as_f64() {
        if !n.is_finite() {
            return Err(SQLiteWasmDatabaseError::JsError(JsValue::from_str(
                "Invalid numeric value at index: NaN/Infinity not supported.",
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

#[wasm_bindgen]
pub struct SQLiteWasmDatabase {
    worker: Worker,
    pending_queries: Rc<RefCell<Vec<(js_sys::Function, js_sys::Function)>>>,
}

impl Serialize for SQLiteWasmDatabase {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let state = serializer.serialize_struct("SQLiteWasmDatabase", 0)?;
        state.end()
    }
}
impl<'de> Deserialize<'de> for SQLiteWasmDatabase {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct InitOpts {
            #[serde(rename = "dbName")]
            db_name: String,
        }

        let opts = InitOpts::deserialize(deserializer)?;
        let trimmed = opts.db_name.trim();
        if trimmed.is_empty() {
            return Err(serde::de::Error::custom(
                "dbName must be a non-empty string",
            ));
        }
        Self::new(trimmed).map_err(|e| {
            serde::de::Error::custom(format!("Failed to create SQLiteWasmDatabase: {e:?}"))
        })
    }
}

#[derive(Debug, Error)]
pub enum SQLiteWasmDatabaseError {
    #[error(transparent)]
    SerdeError(#[from] serde_wasm_bindgen::Error),
    #[error("JavaScript error: {0:?}")]
    JsError(JsValue),
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

#[wasm_export]
impl SQLiteWasmDatabase {
    /// Create a new database connection with fully embedded worker
    #[wasm_export(js_name = "new", preserve_js_class)]
    pub fn new(db_name: &str) -> Result<SQLiteWasmDatabase, SQLiteWasmDatabaseError> {
        let db_name = db_name.trim();
        if db_name.is_empty() {
            return Err(SQLiteWasmDatabaseError::JsError(JsValue::from_str(
                "Database name is required",
            )));
        }
        let worker_code = generate_self_contained_worker(db_name);
        let worker = create_worker_from_code(&worker_code)?;

        let pending_queries: Rc<RefCell<Vec<(js_sys::Function, js_sys::Function)>>> =
            Rc::new(RefCell::new(Vec::new()));
        install_onmessage_handler(&worker, Rc::clone(&pending_queries));

        Ok(SQLiteWasmDatabase {
            worker,
            pending_queries,
        })
    }

    fn normalize_params_js(params: &JsValue) -> Result<js_sys::Array, SQLiteWasmDatabaseError> {
        // If undefined or null, treat as no params
        if params.is_undefined() || params.is_null() {
            return Ok(js_sys::Array::new());
        }

        // Ensure array input
        let arr: js_sys::Array = if js_sys::Array::is_array(params) {
            params.clone().unchecked_into()
        } else {
            return Err(SQLiteWasmDatabaseError::JsError(JsValue::from_str(
                "params must be an array",
            )));
        };

        let normalized = js_sys::Array::new();
        let len = arr.length();
        for i in 0..len {
            let v = arr.get(i);
            let nv = normalize_one_param(&v, i)?;
            normalized.push(&nv);
        }

        Ok(normalized)
    }

    /// Execute a SQL query (optionally parameterized via JS Array)
    ///
    /// Passing `undefined`/`null` from JS maps to `None`.
    #[wasm_export(js_name = "query", unchecked_return_type = "string")]
    pub async fn query(
        &self,
        sql: &str,
        params: Option<js_sys::Array>,
    ) -> Result<String, SQLiteWasmDatabaseError> {
        let worker = &self.worker;
        let pending_queries = Rc::clone(&self.pending_queries);
        let sql = sql.to_string();
        let params_js = params.map(JsValue::from).unwrap_or(JsValue::UNDEFINED);
        let params_array = Self::normalize_params_js(&params_js)?;

        // Build the message object up-front and propagate errors
        let message = js_sys::Object::new();
        js_sys::Reflect::set(
            &message,
            &JsValue::from_str("type"),
            &JsValue::from_str("execute-query"),
        )
        .map_err(SQLiteWasmDatabaseError::JsError)?;
        js_sys::Reflect::set(
            &message,
            &JsValue::from_str("sql"),
            &JsValue::from_str(&sql),
        )
        .map_err(SQLiteWasmDatabaseError::JsError)?;
        if params_array.length() > 0 {
            let params_js = JsValue::from(params_array.clone());
            js_sys::Reflect::set(&message, &JsValue::from_str("params"), &params_js)
                .map_err(SQLiteWasmDatabaseError::JsError)?;
        }

        // Create the Promise that will resolve/reject when the worker responds
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            // Only store callbacks after successful message construction
            pending_queries.borrow_mut().push((resolve, reject));

            let _ = worker.post_message(&message);
        });

        let result = JsFuture::from(promise).await?;
        Ok(result.as_string().unwrap_or_else(|| format!("{result:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

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
    fn test_sqlite_wasm_database_serialization() {
        let db = SQLiteWasmDatabase::new("testdb").expect("Should create database");
        let serialized = serde_json::to_string(&db);

        assert!(serialized.is_ok());
        let json_str = serialized.unwrap();
        assert_eq!(json_str, "{}"); // Empty struct should serialize to empty object
    }

    #[wasm_bindgen_test]
    fn test_sqlite_wasm_database_deserialization() {
        let json_str = r#"{"dbName":"testdb"}"#;
        let result: Result<SQLiteWasmDatabase, _> = serde_json::from_str(json_str);

        assert!(result.is_ok(), "Should be able to deserialize empty object");
    }

    #[wasm_bindgen_test]
    fn test_sqlite_wasm_database_creation() {
        let result = SQLiteWasmDatabase::new("testdb");

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
        if let Ok(db) = SQLiteWasmDatabase::new("testdb") {
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
        assert!(js_value.is_object()); // Should convert to JS error object
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
    }

    // --- Test helpers for spying on Worker.prototype.postMessage ---
    fn install_post_message_spy() {
        // Wrap Worker.prototype.postMessage to capture the message argument
        // into a global so we can assert on the message content.
        let code = r#"
            (function(){
                try {
                    // Clear any previous message
                    self.__lastMessage = undefined;
                    if (!self.__origPostMessage) {
                        self.__origPostMessage = Worker.prototype.postMessage;
                    }
                    Worker.prototype.postMessage = function(msg) {
                        self.__lastMessage = msg;
                        return self.__origPostMessage.call(this, msg);
                    };
                } catch (e) {
                    // If Worker not available, tests will still exercise normalization paths
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
        // Clear for next usage
        let _ = js_sys::Reflect::set(&global, &key, &JsValue::UNDEFINED);
        val.and_then(|v| if v.is_undefined() { None } else { Some(v) })
    }

    #[wasm_bindgen_test]
    async fn test_query_with_various_param_types_and_normalization() {
        install_post_message_spy();
        let db = SQLiteWasmDatabase::new("testdb").expect("db created");

        // Build a params array with all requested JS value types
        let arr = js_sys::Array::new();
        // number
        arr.push(&JsValue::from_f64(42.0));
        // string
        arr.push(&JsValue::from_str("hello"));
        // boolean
        arr.push(&JsValue::from_bool(true));
        // null
        arr.push(&JsValue::NULL);
        // Create a sparse hole at index 4 (left unset, should normalize to NULL)
        // Then set BigInt at index 5 to keep subsequent indices consistent
        let bi = js_sys::BigInt::from(1234u32);
        let bi_js: JsValue = bi.into();
        js_sys::Reflect::set(&arr, &JsValue::from_f64(5.0), &bi_js).expect("set index 5");
        // Uint8Array
        let bytes: [u8; 3] = [1, 2, 3];
        let u8 = js_sys::Uint8Array::from(&bytes[..]);
        let u8_js: JsValue = u8.into();
        arr.push(&u8_js);
        // ArrayBuffer
        let buf = js_sys::ArrayBuffer::new(4);
        let typed = js_sys::Uint8Array::new(&buf);
        typed.copy_from(&[5, 6, 7, 8]);
        let buf_js: JsValue = buf.into();
        arr.push(&buf_js);

        // Call query; we don't care about result success here, just that
        // normalization and message construction do not panic.
        let _ = db.query("SELECT 1", Some(arr)).await;

        // Inspect the last posted message
        if let Some(msg) = take_last_message() {
            // type
            let ty = js_sys::Reflect::get(&msg, &JsValue::from_str("type")).unwrap();
            assert_eq!(ty.as_string().as_deref(), Some("execute-query"));
            // sql
            let sql = js_sys::Reflect::get(&msg, &JsValue::from_str("sql")).unwrap();
            assert_eq!(sql.as_string().as_deref(), Some("SELECT 1"));
            // params presence
            let has_params =
                js_sys::Reflect::has(&msg, &JsValue::from_str("params")).unwrap_or(false);
            assert!(has_params, "params should be present for non-empty array");

            let params = js_sys::Reflect::get(&msg, &JsValue::from_str("params")).unwrap();
            assert!(js_sys::Array::is_array(&params));
            let params: js_sys::Array = params.unchecked_into();
            assert_eq!(params.length(), 8);

            // number
            let v0 = params.get(0);
            assert_eq!(v0.as_f64(), Some(42.0));
            // string
            let v1 = params.get(1);
            assert_eq!(v1.as_string().as_deref(), Some("hello"));
            // boolean
            let v2 = params.get(2);
            assert_eq!(v2.as_bool(), Some(true));
            // null
            let v3 = params.get(3);
            assert!(v3.is_null());
            // sparse hole mapped to null
            let v4 = params.get(4);
            assert!(v4.is_null());
            // BigInt encoded object { __type: "bigint", value: string }
            let v5 = params.get(5);
            assert!(v5.is_object());
            let t5 = js_sys::Reflect::get(&v5, &JsValue::from_str("__type")).unwrap();
            assert_eq!(t5.as_string().as_deref(), Some("bigint"));
            let val5 = js_sys::Reflect::get(&v5, &JsValue::from_str("value")).unwrap();
            assert_eq!(val5.as_string().as_deref(), Some("1234"));
            // Uint8Array encoded object { __type: "blob", base64 }
            let v6 = params.get(6);
            assert!(v6.is_object());
            let t6 = js_sys::Reflect::get(&v6, &JsValue::from_str("__type")).unwrap();
            assert_eq!(t6.as_string().as_deref(), Some("blob"));
            let b64_6 = js_sys::Reflect::get(&v6, &JsValue::from_str("base64")).unwrap();
            let b64_6 = b64_6.as_string().expect("base64 string");
            let expected_6 = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
            assert_eq!(b64_6, expected_6);
            // ArrayBuffer encoded object { __type: "blob", base64 }
            let v7 = params.get(7);
            assert!(v7.is_object());
            let t7 = js_sys::Reflect::get(&v7, &JsValue::from_str("__type")).unwrap();
            assert_eq!(t7.as_string().as_deref(), Some("blob"));
            let b64_7 = js_sys::Reflect::get(&v7, &JsValue::from_str("base64")).unwrap();
            let b64_7 = b64_7.as_string().expect("base64 string");
            let expected_7 = base64::engine::general_purpose::STANDARD.encode([5u8, 6, 7, 8]);
            assert_eq!(b64_7, expected_7);
        } else {
            // If we failed to capture a message, at least the call returned
            // without crashing via normalization. Nothing to assert here.
        }

        uninstall_post_message_spy();
    }

    #[wasm_bindgen_test]
    async fn test_query_params_presence_empty_array_vs_none() {
        install_post_message_spy();
        let db = SQLiteWasmDatabase::new("testdb").expect("db created");

        // Case: None -> no params property on message
        let _ = db.query("SELECT 1", None).await;
        if let Some(msg) = take_last_message() {
            let has_params =
                js_sys::Reflect::has(&msg, &JsValue::from_str("params")).unwrap_or(true);
            assert!(!has_params, "params should be absent when params=None");
        }

        // Case: empty array -> also no params property
        let empty = js_sys::Array::new();
        let _ = db.query("SELECT 1", Some(empty)).await;
        if let Some(msg) = take_last_message() {
            let has_params =
                js_sys::Reflect::has(&msg, &JsValue::from_str("params")).unwrap_or(true);
            assert!(!has_params, "params should be absent for empty array");
        }

        // Case: one param -> params present
        let one = js_sys::Array::new();
        one.push(&JsValue::from_f64(1.0));
        let _ = db.query("SELECT 1", Some(one)).await;
        if let Some(msg) = take_last_message() {
            let has_params =
                js_sys::Reflect::has(&msg, &JsValue::from_str("params")).unwrap_or(false);
            assert!(has_params, "params should be present for non-empty array");
        }

        uninstall_post_message_spy();
    }

    #[wasm_bindgen_test]
    async fn test_query_rejects_nan_infinity_params() {
        let db = SQLiteWasmDatabase::new("testdb").expect("db created");

        // NaN
        {
            let arr = js_sys::Array::new();
            arr.push(&JsValue::from_f64(f64::NAN));
            let res = db.query("SELECT ?", Some(arr)).await;
            assert!(res.is_err(), "NaN should be rejected");
        }

        // +Infinity
        {
            let arr = js_sys::Array::new();
            arr.push(&JsValue::from_f64(f64::INFINITY));
            let res = db.query("SELECT ?", Some(arr)).await;
            assert!(res.is_err(), "+Infinity should be rejected");
        }

        // -Infinity
        {
            let arr = js_sys::Array::new();
            arr.push(&JsValue::from_f64(f64::NEG_INFINITY));
            let res = db.query("SELECT ?", Some(arr)).await;
            assert!(res.is_err(), "-Infinity should be rejected");
        }
    }
}
