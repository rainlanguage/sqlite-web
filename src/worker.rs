// worker.rs - This module runs in the worker context
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{BroadcastChannel, MessageEvent, DedicatedWorkerGlobalScope};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use std::collections::HashMap;
use js_sys::{Promise, Function, Object, Reflect};
use uuid::Uuid;

// Message types for BroadcastChannel communication
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum ChannelMessage {
    #[serde(rename = "new-leader")]
    NewLeader { 
        #[serde(rename = "leaderId")]
        leader_id: String 
    },
    #[serde(rename = "query-request")]
    QueryRequest {
        #[serde(rename = "queryId")]
        query_id: String,
        sql: String,
    },
    #[serde(rename = "query-response")]
    QueryResponse {
        #[serde(rename = "queryId")]
        query_id: String,
        result: Option<String>,
        error: Option<String>,
    },
}

// Messages from main thread
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum WorkerMessage {
    #[serde(rename = "execute-query")]
    ExecuteQuery { sql: String },
}

// Messages to main thread
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum MainThreadMessage {
    #[serde(rename = "query-result")]
    QueryResult {
        result: Option<String>,
        error: Option<String>,
    },
    #[serde(rename = "worker-ready")]
    WorkerReady,
}

// Worker state
struct WorkerState {
    worker_id: String,
    is_leader: Rc<RefCell<bool>>,
    db: Rc<RefCell<Option<SQLiteDatabase>>>,
    channel: BroadcastChannel,
    pending_queries: Rc<RefCell<HashMap<String, PendingQuery>>>,
}

struct PendingQuery {
    resolve: Function,
    reject: Function,
}

use sqlite_wasm_rs::{self as ffi, sahpool_vfs::install as install_opfs_vfs};
use std::ffi::{CString, CStr};

// Real SQLite database using sqlite-wasm-rs FFI
struct SQLiteDatabase {
    db: *mut ffi::sqlite3,
}

unsafe impl Send for SQLiteDatabase {}
unsafe impl Sync for SQLiteDatabase {}

impl SQLiteDatabase {
    async fn initialize_opfs() -> Result<Self, JsValue> {
        web_sys::console::log_1(&"[Worker] Initializing SQLite with OPFS...".into());
        
        // Install OPFS VFS and set as default
        install_opfs_vfs(None, true).await
            .map_err(|e| JsValue::from_str(&format!("Failed to install OPFS VFS: {:?}", e)))?;
        
        // Open database with OPFS
        let mut db = std::ptr::null_mut();
        let db_name = CString::new("opfs-sahpool:worker.db").unwrap();
        
        let ret = unsafe {
            ffi::sqlite3_open_v2(
                db_name.as_ptr(),
                &mut db as *mut _,
                ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE,
                std::ptr::null()
            )
        };
        
        if ret != ffi::SQLITE_OK {
            let error_msg = unsafe {
                let ptr = ffi::sqlite3_errmsg(db);
                if !ptr.is_null() {
                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                } else {
                    format!("SQLite error code: {}", ret)
                }
            };
            return Err(JsValue::from_str(&format!("Failed to open SQLite database: {}", error_msg)));
        }
        
        web_sys::console::log_1(&"[Worker] SQLite database initialized successfully with OPFS".into());
        
        Ok(SQLiteDatabase { db })
    }
    
    async fn exec(&self, sql: &str) -> Result<String, String> {
        web_sys::console::log_1(&format!("[Worker] Executing SQL: {}", sql).into());
        
        let sql_cstr = CString::new(sql).map_err(|e| format!("Invalid SQL string: {}", e))?;
        let mut stmt = std::ptr::null_mut();
        
        // Prepare statement
        let ret = unsafe {
            ffi::sqlite3_prepare_v2(
                self.db,
                sql_cstr.as_ptr(),
                -1,
                &mut stmt,
                std::ptr::null_mut()
            )
        };
        
        if ret != ffi::SQLITE_OK {
            let error_msg = unsafe {
                let ptr = ffi::sqlite3_errmsg(self.db);
                if !ptr.is_null() {
                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                } else {
                    format!("SQLite error code: {}", ret)
                }
            };
            return Err(format!("Failed to prepare statement: {}", error_msg));
        }
        
        // Execute and collect results
        let mut results = Vec::new();
        let mut column_names = Vec::new();
        let mut first_row = true;
        
        loop {
            let step_result = unsafe { ffi::sqlite3_step(stmt) };
            
            match step_result {
                ffi::SQLITE_ROW => {
                    if first_row {
                        // Get column names
                        let col_count = unsafe { ffi::sqlite3_column_count(stmt) };
                        for i in 0..col_count {
                            let col_name = unsafe {
                                let ptr = ffi::sqlite3_column_name(stmt, i);
                                if !ptr.is_null() {
                                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                                } else {
                                    format!("column_{}", i)
                                }
                            };
                            column_names.push(col_name);
                        }
                        first_row = false;
                    }
                    
                    // Get row data
                    let mut row_obj = std::collections::HashMap::new();
                    let col_count = unsafe { ffi::sqlite3_column_count(stmt) };
                    
                    for i in 0..col_count {
                        let col_type = unsafe { ffi::sqlite3_column_type(stmt, i) };
                        let value = match col_type {
                            ffi::SQLITE_INTEGER => {
                                let val = unsafe { ffi::sqlite3_column_int64(stmt, i) };
                                serde_json::Value::Number(serde_json::Number::from(val))
                            }
                            ffi::SQLITE_FLOAT => {
                                let val = unsafe { ffi::sqlite3_column_double(stmt, i) };
                                serde_json::Value::Number(serde_json::Number::from_f64(val).unwrap_or(serde_json::Number::from(0)))
                            }
                            ffi::SQLITE_TEXT => {
                                let ptr = unsafe { ffi::sqlite3_column_text(stmt, i) };
                                if !ptr.is_null() {
                                    let text = unsafe { CStr::from_ptr(ptr as *const i8).to_string_lossy().into_owned() };
                                    serde_json::Value::String(text)
                                } else {
                                    serde_json::Value::Null
                                }
                            }
                            ffi::SQLITE_BLOB => {
                                let len = unsafe { ffi::sqlite3_column_bytes(stmt, i) };
                                serde_json::Value::String(format!("<blob {} bytes>", len))
                            }
                            _ => serde_json::Value::Null,
                        };
                        
                        if let Some(col_name) = column_names.get(i as usize) {
                            row_obj.insert(col_name.clone(), value);
                        }
                    }
                    
                    results.push(serde_json::Value::Object(row_obj.into_iter().collect()));
                }
                ffi::SQLITE_DONE => break,
                _ => {
                    let error_msg = unsafe {
                        let ptr = ffi::sqlite3_errmsg(self.db);
                        if !ptr.is_null() {
                            CStr::from_ptr(ptr).to_string_lossy().into_owned()
                        } else {
                            format!("SQLite error code: {}", step_result)
                        }
                    };
                    unsafe { ffi::sqlite3_finalize(stmt); }
                    return Err(format!("Query execution failed: {}", error_msg));
                }
            }
        }
        
        // Cleanup
        unsafe { ffi::sqlite3_finalize(stmt); }
        
        // Return results
        if sql.trim().to_lowercase().starts_with("select") && !results.is_empty() {
            serde_json::to_string_pretty(&results)
                .map_err(|e| format!("JSON serialization error: {}", e))
        } else if sql.trim().to_lowercase().starts_with("select") {
            Ok("[]".to_string())
        } else {
            // For non-SELECT queries, return changes count
            let changes = unsafe { ffi::sqlite3_changes(self.db) };
            Ok(format!("Query executed successfully. Rows affected: {}", changes))
        }
    }
}

impl Drop for SQLiteDatabase {
    fn drop(&mut self) {
        if !self.db.is_null() {
            unsafe {
                ffi::sqlite3_close(self.db);
            }
        }
    }
}

impl WorkerState {
    fn new() -> Result<Self, JsValue> {
        let worker_id = Uuid::new_v4().to_string();
        let channel = BroadcastChannel::new("sqlite-queries")?;
        
        web_sys::console::log_1(&format!("[Worker {}] Initialized", worker_id).into());
        
        Ok(WorkerState {
            worker_id,
            is_leader: Rc::new(RefCell::new(false)),
            db: Rc::new(RefCell::new(None)),
            channel,
            pending_queries: Rc::new(RefCell::new(HashMap::new())),
        })
    }
    
    fn setup_channel_listener(&self) {
        let is_leader = Rc::clone(&self.is_leader);
        let db = Rc::clone(&self.db);
        let pending_queries = Rc::clone(&self.pending_queries);
        let channel = self.channel.clone();
        
        let onmessage = Closure::wrap(Box::new(move |event: MessageEvent| {
            let data = event.data();
            
            if let Ok(msg) = serde_wasm_bindgen::from_value::<ChannelMessage>(data) {
                match msg {
                    ChannelMessage::QueryRequest { query_id, sql } => {
                        if *is_leader.borrow() {
                            let db = Rc::clone(&db);
                            let channel = channel.clone();
                            
                            spawn_local(async move {
                                let result = if let Some(database) = db.borrow().as_ref() {
                                    database.exec(&sql).await
                                } else {
                                    Err("Database not initialized".to_string())
                                };
                                
                                let response = match result {
                                    Ok(res) => ChannelMessage::QueryResponse {
                                        query_id,
                                        result: Some(res),
                                        error: None,
                                    },
                                    Err(err) => ChannelMessage::QueryResponse {
                                        query_id,
                                        result: None,
                                        error: Some(err),
                                    },
                                };
                                
                                let msg_js = serde_wasm_bindgen::to_value(&response).unwrap();
                                let _ = channel.post_message(&msg_js);
                            });
                        }
                    },
                    ChannelMessage::QueryResponse { query_id, result, error } => {
                        if let Some(pending) = pending_queries.borrow_mut().remove(&query_id) {
                            if let Some(err) = error {
                                let _ = pending.reject.call1(&JsValue::NULL, &JsValue::from_str(&err));
                            } else if let Some(res) = result {
                                let _ = pending.resolve.call1(&JsValue::NULL, &JsValue::from_str(&res));
                            }
                        }
                    },
                    ChannelMessage::NewLeader { leader_id } => {
                        web_sys::console::log_1(&format!("[Worker] New leader: {}", leader_id).into());
                    }
                }
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        
        self.channel.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();
    }
    
    async fn attempt_leadership(&self) {
        let worker_id = self.worker_id.clone();
        let is_leader = Rc::clone(&self.is_leader);
        let db = Rc::clone(&self.db);
        let channel = self.channel.clone();
        
        web_sys::console::log_1(&format!("[Worker {}] Attempting leadership...", worker_id).into());
        
        // Get navigator.locks from WorkerGlobalScope
        let global = js_sys::global();
        let navigator = Reflect::get(&global, &JsValue::from_str("navigator")).unwrap();
        let locks = Reflect::get(&navigator, &JsValue::from_str("locks")).unwrap();
        
        let options = Object::new();
        Reflect::set(&options, &JsValue::from_str("mode"), &JsValue::from_str("exclusive")).unwrap();
        
        let handler = Closure::once(move |_lock: JsValue| -> Promise {
            web_sys::console::log_1(&format!("[Worker {}] Became leader!", worker_id).into());
            
            *is_leader.borrow_mut() = true;
            
            let db = Rc::clone(&db);
            let channel = channel.clone();
            let worker_id = worker_id.clone();
            
            spawn_local(async move {
                match SQLiteDatabase::initialize_opfs().await {
                    Ok(database) => {
                        *db.borrow_mut() = Some(database);
                        
                        let msg = ChannelMessage::NewLeader {
                            leader_id: worker_id.clone(),
                        };
                        let msg_js = serde_wasm_bindgen::to_value(&msg).unwrap();
                        let _ = channel.post_message(&msg_js);
                    }
                    Err(e) => {
                        web_sys::console::error_1(&format!("Failed to initialize DB: {:?}", e).into());
                    }
                }
            });
            
            // Never resolve = hold lock forever
            Promise::new(&mut |_, _| {})
        });
        
        let request_fn = Reflect::get(&locks, &JsValue::from_str("request")).unwrap();
        let request_fn = request_fn.dyn_ref::<Function>().unwrap();
        
        let _ = request_fn.call3(
            &locks,
            &JsValue::from_str("sqlite-database"),
            &options,
            handler.as_ref().unchecked_ref()
        );
        
        handler.forget();
    }
    
    async fn execute_query(&self, sql: String) -> Result<String, String> {
        if *self.is_leader.borrow() {
            if let Some(database) = self.db.borrow().as_ref() {
                database.exec(&sql).await
            } else {
                Err("Database not initialized".to_string())
            }
        } else {
            let query_id = Uuid::new_v4().to_string();
            
            let promise = Promise::new(&mut |resolve, reject| {
                self.pending_queries.borrow_mut().insert(
                    query_id.clone(),
                    PendingQuery { resolve, reject }
                );
            });
            
            let msg = ChannelMessage::QueryRequest {
                query_id: query_id.clone(),
                sql,
            };
            let msg_js = serde_wasm_bindgen::to_value(&msg).unwrap();
            let _ = self.channel.post_message(&msg_js);
            
            // Timeout handling
            let timeout_promise = Promise::new(&mut |_, reject| {
                let query_id = query_id.clone();
                let pending_queries = Rc::clone(&self.pending_queries);
                
                let callback = Closure::once(move || {
                    if pending_queries.borrow_mut().remove(&query_id).is_some() {
                        let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("Query timeout"));
                    }
                });
                
                let global = js_sys::global();
                let set_timeout = Reflect::get(&global, &JsValue::from_str("setTimeout")).unwrap();
                let set_timeout = set_timeout.dyn_ref::<Function>().unwrap();
                set_timeout.call2(&JsValue::NULL, callback.as_ref().unchecked_ref(), &JsValue::from_f64(5000.0)).unwrap();
                callback.forget();
            });
            
            let result = wasm_bindgen_futures::JsFuture::from(
                js_sys::Promise::race(&js_sys::Array::of2(&promise, &timeout_promise))
            ).await;
            
            match result {
                Ok(val) => {
                    if let Some(s) = val.as_string() {
                        Ok(s)
                    } else {
                        Err("Invalid response".to_string())
                    }
                }
                Err(e) => Err(format!("{:?}", e))
            }
        }
    }
}

// Global state
thread_local! {
    static WORKER_STATE: RefCell<Option<Rc<WorkerState>>> = RefCell::new(None);
}

/// Entry point for the worker - called from the blob
#[wasm_bindgen]
pub fn worker_main() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    
    web_sys::console::log_1(&"[Worker] Starting worker_main...".into());
    
    let state = Rc::new(WorkerState::new()?);
    
    state.setup_channel_listener();
    
    let state_clone = Rc::clone(&state);
    spawn_local(async move {
        state_clone.attempt_leadership().await;
    });
    
    WORKER_STATE.with(|s| {
        *s.borrow_mut() = Some(Rc::clone(&state));
    });
    
    // Setup message handler from main thread
    let global = js_sys::global();
    let worker_scope: DedicatedWorkerGlobalScope = global.unchecked_into();
    
    let onmessage = Closure::wrap(Box::new(move |event: MessageEvent| {
        let data = event.data();
        
        // Parse JavaScript message directly
        if let Ok(msg_type) = js_sys::Reflect::get(&data, &JsValue::from_str("type")) {
            if let Some(type_str) = msg_type.as_string() {
                if type_str == "execute-query" {
                    if let Ok(sql_val) = js_sys::Reflect::get(&data, &JsValue::from_str("sql")) {
                        if let Some(sql) = sql_val.as_string() {
                            WORKER_STATE.with(|s| {
                                if let Some(state) = s.borrow().as_ref() {
                                    let state = Rc::clone(state);
                                    spawn_local(async move {
                                        let result = state.execute_query(sql).await;
                                        
                                        // Send response as plain JavaScript object
                                        let response = js_sys::Object::new();
                                        js_sys::Reflect::set(&response, &JsValue::from_str("type"), &JsValue::from_str("query-result")).unwrap();
                                        
                                        match result {
                                            Ok(res) => {
                                                js_sys::Reflect::set(&response, &JsValue::from_str("result"), &JsValue::from_str(&res)).unwrap();
                                                js_sys::Reflect::set(&response, &JsValue::from_str("error"), &JsValue::NULL).unwrap();
                                            },
                                            Err(err) => {
                                                js_sys::Reflect::set(&response, &JsValue::from_str("result"), &JsValue::NULL).unwrap();
                                                js_sys::Reflect::set(&response, &JsValue::from_str("error"), &JsValue::from_str(&err)).unwrap();
                                            }
                                        };
                                        
                                        let global = js_sys::global();
                                        let worker_scope: DedicatedWorkerGlobalScope = global.unchecked_into();
                                        let _ = worker_scope.post_message(&response);
                                    });
                                }
                            });
                        }
                    }
                }
            }
        }
    }) as Box<dyn FnMut(MessageEvent)>);
    
    worker_scope.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();
    
    Ok(())
}