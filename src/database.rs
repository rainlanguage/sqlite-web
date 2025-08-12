use sqlite_wasm_rs::export::{*, install_opfs_sahpool};
use std::ffi::{CStr, CString};
use wasm_bindgen::prelude::*;
use crate::database_functions::register_custom_functions;

// Real SQLite database using sqlite-wasm-rs FFI
pub struct SQLiteDatabase {
    db: *mut sqlite3,
}

unsafe impl Send for SQLiteDatabase {}
unsafe impl Sync for SQLiteDatabase {}

impl SQLiteDatabase {
    pub async fn initialize_opfs() -> Result<Self, JsValue> {
        web_sys::console::log_1(&"[Worker] Initializing SQLite with OPFS...".into());

        // Install OPFS VFS and set as default
        install_opfs_sahpool(None, true)
            .await
            .map_err(|e| JsValue::from_str(&format!("Failed to install OPFS VFS: {:?}", e)))?;

        // Open database with OPFS
        let mut db = std::ptr::null_mut();
        let db_name = CString::new("opfs-sahpool:worker.db").unwrap();

        let ret = unsafe {
            sqlite3_open_v2(
                db_name.as_ptr(),
                &mut db as *mut _,
                SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
                std::ptr::null(),
            )
        };

        if ret != SQLITE_OK {
            let error_msg = unsafe {
                let ptr = sqlite3_errmsg(db);
                if !ptr.is_null() {
                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                } else {
                    format!("SQLite error code: {}", ret)
                }
            };
            return Err(JsValue::from_str(&format!(
                "Failed to open SQLite database: {}",
                error_msg
            )));
        }

        web_sys::console::log_1(
            &"[Worker] SQLite database initialized successfully with OPFS".into(),
        );

        // Register custom functions
        register_custom_functions(db).map_err(|e| JsValue::from_str(&e))?;

        web_sys::console::log_1(&"[Worker] Custom functions registered".into());

        Ok(SQLiteDatabase { db })
    }

    pub async fn exec(&self, sql: &str) -> Result<String, String> {
        web_sys::console::log_1(&format!("[Worker] Executing SQL: {}", sql).into());

        let sql_cstr = CString::new(sql).map_err(|e| format!("Invalid SQL string: {}", e))?;
        let mut stmt = std::ptr::null_mut();

        // Prepare statement
        let ret = unsafe {
            sqlite3_prepare_v2(
                self.db,
                sql_cstr.as_ptr(),
                -1,
                &mut stmt,
                std::ptr::null_mut(),
            )
        };

        if ret != SQLITE_OK {
            let error_msg = unsafe {
                let ptr = sqlite3_errmsg(self.db);
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
            let step_result = unsafe { sqlite3_step(stmt) };

            match step_result {
                SQLITE_ROW => {
                    if first_row {
                        // Get column names
                        let col_count = unsafe { sqlite3_column_count(stmt) };
                        for i in 0..col_count {
                            let col_name = unsafe {
                                let ptr = sqlite3_column_name(stmt, i);
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
                    let col_count = unsafe { sqlite3_column_count(stmt) };

                    for i in 0..col_count {
                        let col_type = unsafe { sqlite3_column_type(stmt, i) };
                        let value = match col_type {
                            SQLITE_INTEGER => {
                                let val = unsafe { sqlite3_column_int64(stmt, i) };
                                serde_json::Value::Number(serde_json::Number::from(val))
                            }
                            SQLITE_FLOAT => {
                                let val = unsafe { sqlite3_column_double(stmt, i) };
                                serde_json::Value::Number(
                                    serde_json::Number::from_f64(val)
                                        .unwrap_or(serde_json::Number::from(0)),
                                )
                            }
                            SQLITE_TEXT => {
                                let ptr = unsafe { sqlite3_column_text(stmt, i) };
                                if !ptr.is_null() {
                                    let text = unsafe {
                                        CStr::from_ptr(ptr as *const i8)
                                            .to_string_lossy()
                                            .into_owned()
                                    };
                                    serde_json::Value::String(text)
                                } else {
                                    serde_json::Value::Null
                                }
                            }
                            SQLITE_BLOB => {
                                let len = unsafe { sqlite3_column_bytes(stmt, i) };
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
                SQLITE_DONE => break,
                _ => {
                    let error_msg = unsafe {
                        let ptr = sqlite3_errmsg(self.db);
                        if !ptr.is_null() {
                            CStr::from_ptr(ptr).to_string_lossy().into_owned()
                        } else {
                            format!("SQLite error code: {}", step_result)
                        }
                    };
                    unsafe {
                        sqlite3_finalize(stmt);
                    }
                    return Err(format!("Query execution failed: {}", error_msg));
                }
            }
        }

        // Cleanup
        unsafe {
            sqlite3_finalize(stmt);
        }

        // Return results
        if sql.trim().to_lowercase().starts_with("select") && !results.is_empty() {
            serde_json::to_string_pretty(&results)
                .map_err(|e| format!("JSON serialization error: {}", e))
        } else if sql.trim().to_lowercase().starts_with("select") {
            Ok("[]".to_string())
        } else {
            // For non-SELECT queries, return changes count
            let changes = unsafe { sqlite3_changes(self.db) };
            Ok(format!(
                "Query executed successfully. Rows affected: {}",
                changes
            ))
        }
    }
}

impl Drop for SQLiteDatabase {
    fn drop(&mut self) {
        if !self.db.is_null() {
            unsafe {
                sqlite3_close(self.db);
            }
        }
    }
}
