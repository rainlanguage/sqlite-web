use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use js_sys::Array;
use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_utils::prelude::*;
use web_sys::Worker;

use crate::errors::SQLiteWasmDatabaseError;
use crate::params::normalize_params_js;
use crate::ready::{InitializationState, ReadySignal};
use crate::utils::describe_js_value;
use crate::worker::{create_worker_from_code, install_onmessage_handler};
use crate::worker_template::generate_self_contained_worker;

#[wasm_bindgen]
pub struct SQLiteWasmDatabase {
    worker: Worker,
    pending_queries: Rc<RefCell<HashMap<u32, (js_sys::Function, js_sys::Function)>>>,
    next_request_id: Rc<RefCell<u32>>,
    ready_signal: ReadySignal,
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

#[wasm_export]
impl SQLiteWasmDatabase {
    /// Create a new database connection with fully embedded worker
    #[wasm_export(js_name = "new", preserve_js_class)]
    pub async fn new(db_name: &str) -> Result<SQLiteWasmDatabase, SQLiteWasmDatabaseError> {
        let db_name = db_name.trim();
        if db_name.is_empty() {
            return Err(SQLiteWasmDatabaseError::JsError(JsValue::from_str(
                "Database name is required",
            )));
        }
        let db = Self::construct(db_name)?;
        db.wait_until_ready().await?;
        Ok(db)
    }

    fn construct(db_name: &str) -> Result<SQLiteWasmDatabase, SQLiteWasmDatabaseError> {
        let worker_code = generate_self_contained_worker(db_name);
        let worker = create_worker_from_code(&worker_code)?;

        let pending_queries: Rc<RefCell<HashMap<u32, (js_sys::Function, js_sys::Function)>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let ready_signal = ReadySignal::new();
        install_onmessage_handler(&worker, Rc::clone(&pending_queries), ready_signal.clone());
        let next_request_id = Rc::new(RefCell::new(1u32));

        Ok(SQLiteWasmDatabase {
            worker,
            pending_queries,
            next_request_id,
            ready_signal,
        })
    }

    fn normalize_params(params: Option<Array>) -> Result<Array, SQLiteWasmDatabaseError> {
        let params_js = params.map(JsValue::from).unwrap_or(JsValue::UNDEFINED);
        normalize_params_js(&params_js)
    }

    async fn wait_until_ready(&self) -> Result<(), SQLiteWasmDatabaseError> {
        match self.ready_signal.current_state() {
            InitializationState::Ready => return Ok(()),
            InitializationState::Failed(reason) => {
                return Err(SQLiteWasmDatabaseError::InitializationFailed(reason));
            }
            InitializationState::Pending => {}
        }

        let promise = self.ready_signal.wait_promise()?;

        match JsFuture::from(promise).await {
            Ok(_) => match self.ready_signal.current_state() {
                InitializationState::Ready => Ok(()),
                InitializationState::Failed(reason) => {
                    Err(SQLiteWasmDatabaseError::InitializationFailed(reason))
                }
                InitializationState::Pending => Err(SQLiteWasmDatabaseError::InitializationFailed(
                    "Worker failed to signal readiness".to_string(),
                )),
            },
            Err(err) => {
                let reason = describe_js_value(&err);
                self.ready_signal.mark_failed(reason.clone());
                Err(SQLiteWasmDatabaseError::InitializationFailed(reason))
            }
        }
    }

    /// Execute a SQL query (optionally parameterized via JS Array)
    ///
    /// Passing `undefined`/`null` from JS maps to `None`.
    #[wasm_export(js_name = "query", unchecked_return_type = "string")]
    pub async fn query(
        &self,
        sql: &str,
        params: Option<Array>,
    ) -> Result<String, SQLiteWasmDatabaseError> {
        let worker = &self.worker;
        let pending_queries = Rc::clone(&self.pending_queries);
        let sql = sql.to_string();
        let params_array = Self::normalize_params(params)?;

        if let InitializationState::Failed(reason) = self.ready_signal.current_state() {
            return Err(SQLiteWasmDatabaseError::InitializationFailed(reason));
        }

        let message = js_sys::Object::new();
        js_sys::Reflect::set(
            &message,
            &JsValue::from_str("type"),
            &JsValue::from_str("execute-query"),
        )
        .map_err(SQLiteWasmDatabaseError::JsError)?;

        let request_id = {
            let mut n = self.next_request_id.borrow_mut();
            let id = *n;
            *n = n.wrapping_add(1).max(1);
            id
        };
        js_sys::Reflect::set(
            &message,
            &JsValue::from_str("requestId"),
            &JsValue::from_f64(request_id as f64),
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

        let rid_for_insert = request_id;
        let promise =
            js_sys::Promise::new(&mut |resolve, reject| match worker.post_message(&message) {
                Ok(()) => {
                    pending_queries
                        .borrow_mut()
                        .insert(rid_for_insert, (resolve, reject));
                }
                Err(err) => {
                    let _ = reject.call1(&JsValue::NULL, &err);
                }
            });

        let result = match JsFuture::from(promise).await {
            Ok(value) => value,
            Err(err) => {
                if err
                    .as_string()
                    .as_deref()
                    .map(|s| s == "InitializationPending")
                    .unwrap_or(false)
                {
                    return Err(SQLiteWasmDatabaseError::InitializationPending);
                }
                return Err(SQLiteWasmDatabaseError::JsError(err));
            }
        };
        Ok(result.as_string().unwrap_or_else(|| format!("{result:?}")))
    }
}
