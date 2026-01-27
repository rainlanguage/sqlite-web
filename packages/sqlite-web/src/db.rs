use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use js_sys::{Array, Reflect};
use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_utils::prelude::*;
use web_sys::Worker;

use crate::errors::SQLiteWasmDatabaseError;
use crate::messages::WORKER_ERROR_TYPE_INITIALIZATION_PENDING;
use crate::opfs::delete_opfs_sahpool_directory;
use crate::params::normalize_params_js;
use crate::ready::{InitializationState, ReadySignal};
use crate::utils::describe_js_value;
use crate::worker::{create_worker_from_code, install_onmessage_handler};
use crate::worker_template::generate_self_contained_worker;

#[wasm_bindgen]
pub struct SQLiteWasmDatabase {
    worker: Rc<RefCell<Worker>>,
    db_name: String,
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
            worker: Rc::new(RefCell::new(worker)),
            db_name: db_name.to_string(),
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
        let worker = Rc::clone(&self.worker);
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
        let promise = js_sys::Promise::new(&mut |resolve, reject| match worker
            .borrow()
            .post_message(&message)
        {
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
            Err(err) if is_initialization_pending_error(&err) => {
                return Err(SQLiteWasmDatabaseError::InitializationPending);
            }
            Err(err) => {
                return Err(SQLiteWasmDatabaseError::JsError(err));
            }
        };
        Ok(result.as_string().unwrap_or_else(|| format!("{result:?}")))
    }

    #[wasm_export(js_name = "wipeAndRecreate", unchecked_return_type = "void")]
    pub async fn wipe_and_recreate(&self) -> Result<(), SQLiteWasmDatabaseError> {
        self.worker.borrow().terminate();

        for (_, (_, reject)) in self.pending_queries.borrow_mut().drain() {
            let err = JsValue::from_str("Database wipe in progress");
            let _ = reject.call1(&JsValue::NULL, &err);
        }

        self.ready_signal.reset();

        delete_opfs_sahpool_directory().await?;

        let worker_code = generate_self_contained_worker(&self.db_name);
        let new_worker =
            create_worker_from_code(&worker_code).map_err(SQLiteWasmDatabaseError::JsError)?;

        install_onmessage_handler(
            &new_worker,
            Rc::clone(&self.pending_queries),
            self.ready_signal.clone(),
        );

        *self.worker.borrow_mut() = new_worker;

        self.wait_until_ready().await?;

        Ok(())
    }
}

fn is_initialization_pending_error(err: &JsValue) -> bool {
    let error_type = Reflect::get(err, &JsValue::from_str("type"))
        .ok()
        .and_then(|value| value.as_string());
    if error_type.as_deref() == Some(WORKER_ERROR_TYPE_INITIALIZATION_PENDING) {
        return true;
    }
    err.as_string().as_deref() == Some(WORKER_ERROR_TYPE_INITIALIZATION_PENDING)
}

#[cfg(all(test, target_family = "wasm"))]
mod tests {
    use super::*;
    use base64::Engine;
    use js_sys::{Array, ArrayBuffer, BigInt, Object, Uint8Array};
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn normalize_params_handles_none_and_empty_arrays() {
        let empty = SQLiteWasmDatabase::normalize_params(None).expect("None => empty array");
        assert_eq!(empty.length(), 0);

        let arr = Array::new();
        let normalized =
            SQLiteWasmDatabase::normalize_params(Some(arr)).expect("empty array stays empty");
        assert_eq!(normalized.length(), 0);
    }

    #[wasm_bindgen_test]
    fn normalize_params_normalizes_mixed_values() {
        let params = Array::new();
        params.push(&JsValue::from_f64(123.0));
        params.push(&JsValue::from_str("hey"));
        params.push(&JsValue::from_bool(true));
        params.push(&JsValue::NULL);
        let bi: JsValue = BigInt::from(77u32).into();
        params.push(&bi);
        let buf = ArrayBuffer::new(2);
        Uint8Array::new(&buf).copy_from(&[9u8, 10]);
        let buf_js: JsValue = buf.into();
        params.push(&buf_js);

        let normalized =
            SQLiteWasmDatabase::normalize_params(Some(params)).expect("normalization works");
        assert_eq!(normalized.length(), 6);
        assert_eq!(normalized.get(0).as_f64(), Some(123.0));
        assert_eq!(normalized.get(1).as_string().as_deref(), Some("hey"));
        assert_eq!(normalized.get(2).as_bool(), Some(true));
        assert!(normalized.get(3).is_null());

        let bigint = normalized.get(4);
        assert_eq!(
            js_sys::Reflect::get(&bigint, &JsValue::from_str("__type"))
                .unwrap()
                .as_string()
                .as_deref(),
            Some("bigint")
        );
        assert_eq!(
            js_sys::Reflect::get(&bigint, &JsValue::from_str("value"))
                .unwrap()
                .as_string()
                .as_deref(),
            Some("77")
        );

        let blob = normalized.get(5);
        assert_eq!(
            js_sys::Reflect::get(&blob, &JsValue::from_str("__type"))
                .unwrap()
                .as_string()
                .as_deref(),
            Some("blob")
        );
        let actual = js_sys::Reflect::get(&blob, &JsValue::from_str("base64"))
            .unwrap()
            .as_string()
            .expect("base64 string present");
        let expected = base64::engine::general_purpose::STANDARD.encode([9u8, 10]);
        assert_eq!(actual, expected);
    }

    #[wasm_bindgen_test(async)]
    async fn new_rejects_blank_database_name() {
        let err = match SQLiteWasmDatabase::new("   ").await {
            Ok(_) => panic!("blank names should be rejected before constructing worker"),
            Err(err) => err,
        };
        match err {
            SQLiteWasmDatabaseError::JsError(js) => {
                assert_eq!(js.as_string().as_deref(), Some("Database name is required"))
            }
            other => panic!("expected JsError, got {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn detects_structured_initialization_pending_errors() {
        let err = Object::new();
        let _ = js_sys::Reflect::set(
            &err,
            &JsValue::from_str("type"),
            &JsValue::from_str(WORKER_ERROR_TYPE_INITIALIZATION_PENDING),
        );
        let js_val: JsValue = err.into();
        assert!(is_initialization_pending_error(&js_val));
    }

    #[wasm_bindgen_test]
    fn detects_string_initialization_pending_errors() {
        let js_val = JsValue::from_str(WORKER_ERROR_TYPE_INITIALIZATION_PENDING);
        assert!(is_initialization_pending_error(&js_val));
    }

    #[wasm_bindgen_test(async)]
    async fn wipe_and_recreate_tests() {
        let db = SQLiteWasmDatabase::new("test_wipe").await.unwrap();
        db.wipe_and_recreate().await.unwrap();

        db.query(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
            None,
        )
        .await
        .unwrap();
        db.query("INSERT INTO users (name) VALUES ('Alice')", None)
            .await
            .unwrap();

        let result = db
            .query("SELECT COUNT(*) as count FROM users", None)
            .await
            .unwrap();
        assert!(result.contains("\"count\": 1"));

        db.wipe_and_recreate().await.unwrap();

        let result = db.query("SELECT * FROM users", None).await;
        assert!(result.is_err() || result.unwrap().contains("no such table"));

        let create_result = db
            .query(
                "CREATE TABLE new_table (id INTEGER PRIMARY KEY, value TEXT)",
                None,
            )
            .await;
        assert!(create_result.is_ok());

        let insert_result = db
            .query("INSERT INTO new_table (value) VALUES ('test')", None)
            .await;
        assert!(insert_result.is_ok());

        let select_result = db.query("SELECT * FROM new_table", None).await.unwrap();
        assert!(select_result.contains("test"));

        for i in 0..3 {
            db.query(&format!("CREATE TABLE t{} (id INTEGER)", i), None)
                .await
                .unwrap();
            db.wipe_and_recreate().await.unwrap();
        }

        let result = db
            .query("SELECT name FROM sqlite_master WHERE type='table'", None)
            .await
            .unwrap();
        assert!(!result.contains("t0"));
        assert!(!result.contains("t1"));
        assert!(!result.contains("t2"));

        let arr = Array::new();
        arr.push(&JsValue::from_f64(f64::NAN));
        let res = db.query("SELECT ?", Some(arr)).await;
        assert!(res.is_err(), "NaN should be rejected");

        let arr = Array::new();
        arr.push(&JsValue::from_f64(f64::INFINITY));
        let res = db.query("SELECT ?", Some(arr)).await;
        assert!(res.is_err(), "+Infinity should be rejected");

        let arr = Array::new();
        arr.push(&JsValue::from_f64(f64::NEG_INFINITY));
        let res = db.query("SELECT ?", Some(arr)).await;
        assert!(res.is_err(), "-Infinity should be rejected");
    }
}
