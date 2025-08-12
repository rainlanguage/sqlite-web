use js_sys::Array;
use serde::{de::IgnoredAny, Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use thiserror::Error;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_utils::prelude::*;
use web_sys::{Blob, BlobPropertyBag, MessageEvent, Url, Worker};

mod worker_template;
use worker_template::generate_self_contained_worker;

#[wasm_bindgen]
pub struct DatabaseConnection {
    worker: Worker,
    pending_queries: Rc<RefCell<Vec<(js_sys::Function, js_sys::Function)>>>,
}

impl Serialize for DatabaseConnection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let state = serializer.serialize_struct("DatabaseConnection", 0)?;
        state.end()
    }
}
impl<'de> Deserialize<'de> for DatabaseConnection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let _ = deserializer.deserialize_any(IgnoredAny)?;
        Self::new().map_err(|e| {
            serde::de::Error::custom(format!("Failed to create DatabaseConnection: {:?}", e))
        })
    }
}

#[derive(Debug, Error)]
pub enum DatabaseConnectionError {
    #[error(transparent)]
    SerdeError(#[from] serde_wasm_bindgen::Error),
    #[error("JavaScript error: {0:?}")]
    JsError(JsValue),
}

impl From<JsValue> for DatabaseConnectionError {
    fn from(value: JsValue) -> Self {
        DatabaseConnectionError::JsError(value)
    }
}

impl From<DatabaseConnectionError> for JsValue {
    fn from(value: DatabaseConnectionError) -> Self {
        JsError::new(&value.to_string()).into()
    }
}

impl From<DatabaseConnectionError> for WasmEncodedError {
    fn from(value: DatabaseConnectionError) -> Self {
        WasmEncodedError {
            msg: value.to_string(),
            readable_msg: value.to_string(),
        }
    }
}

#[wasm_export]
impl DatabaseConnection {
    /// Create a new database connection with fully embedded worker
    #[wasm_export(js_name = "new", preserve_js_class)]
    pub fn new() -> Result<DatabaseConnection, DatabaseConnectionError> {
        web_sys::console::log_1(&"Creating self-contained worker...".into());

        // Create the worker with embedded WASM and glue code
        let worker_code = generate_self_contained_worker();

        // Create a Blob with the worker code
        let blob_parts = Array::new();
        blob_parts.push(&JsValue::from_str(&worker_code));

        let blob_options = BlobPropertyBag::new();
        blob_options.set_type("application/javascript");

        let blob = Blob::new_with_str_sequence_and_options(&blob_parts, &blob_options)?;

        // Create a blob URL
        let worker_url = Url::create_object_url_with_blob(&blob)?;

        // Create the worker from the blob URL
        let worker = Worker::new(&worker_url)?;

        // Clean up the blob URL
        Url::revoke_object_url(&worker_url)?;

        let pending_queries: Rc<RefCell<Vec<(js_sys::Function, js_sys::Function)>>> =
            Rc::new(RefCell::new(Vec::new()));
        let pending_queries_clone = Rc::clone(&pending_queries);

        // Setup message handler
        let onmessage = Closure::wrap(Box::new(move |event: MessageEvent| {
            let data = event.data();

            // Handle worker ready message
            if let Ok(obj) = js_sys::Reflect::get(&data, &JsValue::from_str("type")) {
                if let Some(msg_type) = obj.as_string() {
                    if msg_type == "worker-ready" {
                        web_sys::console::log_1(&"✅ Worker is ready!".into());
                        return;
                    } else if msg_type == "worker-error" {
                        if let Ok(error) = js_sys::Reflect::get(&data, &JsValue::from_str("error"))
                        {
                            web_sys::console::error_1(
                                &format!("❌ Worker error: {:?}", error).into(),
                            );
                        }
                        return;
                    }
                }
            }

            // Handle query responses - parse JavaScript objects directly
            if let Ok(obj) = js_sys::Reflect::get(&data, &JsValue::from_str("type")) {
                if let Some(msg_type) = obj.as_string() {
                    if msg_type == "query-result" {
                        if let Some((resolve, reject)) = pending_queries_clone.borrow_mut().pop() {
                            // Check for error first
                            if let Ok(error) =
                                js_sys::Reflect::get(&data, &JsValue::from_str("error"))
                            {
                                if !error.is_null() && !error.is_undefined() {
                                    let error_str =
                                        error.as_string().unwrap_or_else(|| format!("{:?}", error));
                                    let _ = reject
                                        .call1(&JsValue::NULL, &JsValue::from_str(&error_str));
                                    return;
                                }
                            }

                            // Handle successful result
                            if let Ok(result) =
                                js_sys::Reflect::get(&data, &JsValue::from_str("result"))
                            {
                                if !result.is_null() && !result.is_undefined() {
                                    let result_str = result
                                        .as_string()
                                        .unwrap_or_else(|| format!("{:?}", result));
                                    let _ = resolve
                                        .call1(&JsValue::NULL, &JsValue::from_str(&result_str));
                                }
                            }
                        }
                    }
                }
            }
        }) as Box<dyn FnMut(MessageEvent)>);

        worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();

        Ok(DatabaseConnection {
            worker,
            pending_queries,
        })
    }

    /// Execute a SQL query
    #[wasm_export(js_name = "query", unchecked_return_type = "string")]
    pub async fn query(&self, sql: &str) -> Result<String, DatabaseConnectionError> {
        let worker = &self.worker;
        let pending_queries = Rc::clone(&self.pending_queries);
        let sql = sql.to_string();

        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            // Store the promise callbacks
            pending_queries.borrow_mut().push((resolve, reject));

            // Send query to worker - create JavaScript object directly
            let message = js_sys::Object::new();
            js_sys::Reflect::set(
                &message,
                &JsValue::from_str("type"),
                &JsValue::from_str("execute-query"),
            )
            .unwrap();
            js_sys::Reflect::set(
                &message,
                &JsValue::from_str("sql"),
                &JsValue::from_str(&sql),
            )
            .unwrap();

            let _ = worker.post_message(&message);
        });

        let result = JsFuture::from(promise).await?;
        Ok(result
            .as_string()
            .unwrap_or_else(|| format!("{:?}", result)))
    }
}
