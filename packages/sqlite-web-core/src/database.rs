use crate::database_functions::register_custom_functions;
use crate::util::sanitize_db_filename;
use base64::Engine;
use sqlite_wasm_rs::export::{install_opfs_sahpool, *};
use std::ffi::{CStr, CString};
use std::os::raw::c_void;
use wasm_bindgen::prelude::*;

// Real SQLite database using sqlite-wasm-rs FFI
pub struct SQLiteDatabase {
    db: *mut sqlite3,
    in_transaction: bool,
}

unsafe impl Send for SQLiteDatabase {}
unsafe impl Sync for SQLiteDatabase {}

struct BoundBuffers {
    _texts: Vec<CString>,
    _blobs: Vec<Vec<u8>>,
}

struct StmtGuard {
    stmt: *mut sqlite3_stmt,
}

impl StmtGuard {
    fn new(stmt: *mut sqlite3_stmt) -> Self {
        Self { stmt }
    }

    fn take(&mut self) -> *mut sqlite3_stmt {
        let s = self.stmt;
        self.stmt = std::ptr::null_mut();
        s
    }
}

impl Drop for StmtGuard {
    fn drop(&mut self) {
        if !self.stmt.is_null() {
            unsafe { sqlite3_finalize(self.stmt) };
            self.stmt = std::ptr::null_mut();
        }
    }
}

// Placeholder detection mode used during parameter binding
enum PlaceholderMode {
    Plain {
        count: usize,
    },
    Numbered {
        max: usize,
        present: std::collections::BTreeSet<usize>,
    },
}

// Normalized parameter kinds for binding
enum ParamKind {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Text(CString),
    Blob(Vec<u8>),
}

impl SQLiteDatabase {
    fn refresh_transaction_state(&mut self) {
        self.in_transaction = unsafe { sqlite3_get_autocommit(self.db) } == 0;
    }

    async fn rollback_if_in_transaction(&self) {
        if unsafe { sqlite3_get_autocommit(self.db) } == 0 {
            let _ = self.exec_single_statement("ROLLBACK").await;
        }
    }

    fn prepare_one(
        &self,
        ptr: *const i8,
    ) -> Result<(Option<*mut sqlite3_stmt>, *const i8), String> {
        let mut stmt: *mut sqlite3_stmt = std::ptr::null_mut();
        let mut tail: *const i8 = std::ptr::null();
        let ret = unsafe { sqlite3_prepare_v2(self.db, ptr, -1, &mut stmt, &mut tail) };
        if ret != SQLITE_OK {
            // If sqlite3_prepare_v2 returns an error, stmt may still be non-null.
            // Finalize it to avoid leaking resources or retaining locks.
            if !stmt.is_null() {
                unsafe {
                    let _ = sqlite3_finalize(stmt);
                }
            }
            let msg = self.sqlite_errmsg();
            let detail = if msg == "Unknown SQLite error" {
                format!("SQLite error code: {ret}")
            } else {
                msg
            };
            return Err(format!("Failed to prepare statement: {detail}"));
        }
        Ok((if stmt.is_null() { None } else { Some(stmt) }, tail))
    }
    fn sqlite_errmsg(&self) -> String {
        unsafe {
            let p = sqlite3_errmsg(self.db);
            if !p.is_null() {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            } else {
                // Fallback when SQLite does not provide an error message
                "Unknown SQLite error".to_string()
            }
        }
    }

    fn collect_column_names(stmt: *mut sqlite3_stmt) -> Vec<String> {
        let col_count = unsafe { sqlite3_column_count(stmt) };
        let mut names = Vec::with_capacity(col_count as usize);
        for i in 0..col_count {
            let col_name = unsafe {
                let ptr = sqlite3_column_name(stmt, i);
                if !ptr.is_null() {
                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                } else {
                    format!("column_{i}")
                }
            };
            names.push(col_name);
        }
        names
    }

    fn read_column_value(stmt: *mut sqlite3_stmt, i: i32) -> serde_json::Value {
        let col_type = unsafe { sqlite3_column_type(stmt, i) };
        match col_type {
            SQLITE_INTEGER => {
                let val = unsafe { sqlite3_column_int64(stmt, i) };
                serde_json::Value::Number(serde_json::Number::from(val))
            }
            SQLITE_FLOAT => {
                let val = unsafe { sqlite3_column_double(stmt, i) };
                if val.is_finite() {
                    // Safe to unwrap: serde_json rejects only non-finite floats
                    serde_json::Value::Number(serde_json::Number::from_f64(val).unwrap())
                } else {
                    serde_json::Value::Null
                }
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
                serde_json::Value::String(format!("<blob {len} bytes>"))
            }
            _ => serde_json::Value::Null,
        }
    }

    fn detect_placeholder_mode(&self, stmt: *mut sqlite3_stmt) -> Result<PlaceholderMode, String> {
        let param_count = unsafe { sqlite3_bind_parameter_count(stmt) } as usize;
        let mut has_plain = false;
        let mut has_numbered = false;
        let mut has_named = false;
        let mut numbered_max: usize = 0;
        let mut numbered_present = std::collections::BTreeSet::new();

        for i in 1..=param_count as i32 {
            let name_ptr = unsafe { sqlite3_bind_parameter_name(stmt, i) };
            if name_ptr.is_null() {
                has_plain = true;
            } else {
                let name = unsafe { CStr::from_ptr(name_ptr) }.to_string_lossy();
                let s = name.as_ref();
                if let Some(digits) = s.strip_prefix('?') {
                    if digits.is_empty() {
                        has_plain = true;
                    } else if let Ok(n) = digits.parse::<isize>() {
                        if n <= 0 {
                            return Err(
                                "Invalid parameter index: ?0 or negative indices are not allowed."
                                    .to_string(),
                            );
                        }
                        has_numbered = true;
                        let n_u = n as usize;
                        numbered_present.insert(n_u);
                        if n_u > numbered_max {
                            numbered_max = n_u;
                        }
                    } else {
                        return Err(format!("Invalid parameter index: {}", s));
                    }
                } else {
                    has_named = true; // :name/@name/$name not supported
                }
            }
        }

        if has_named {
            return Err("Named parameters not supported.".to_string());
        }
        if has_plain && has_numbered {
            return Err("Mixing '?' and '?N' placeholders is not supported.".to_string());
        }

        if has_plain {
            Ok(PlaceholderMode::Plain { count: param_count })
        } else {
            Ok(PlaceholderMode::Numbered {
                max: numbered_max,
                present: numbered_present,
            })
        }
    }

    fn validate_params_against_mode(
        &self,
        mode: &PlaceholderMode,
        params_len: usize,
    ) -> Result<(), String> {
        match mode {
            PlaceholderMode::Plain { count } => {
                if params_len != *count {
                    return Err(format!(
                        "Expected {} parameters but got {}.",
                        count, params_len
                    ));
                }
                Ok(())
            }
            PlaceholderMode::Numbered { max, present } => {
                for need in 1..=*max {
                    if !present.contains(&need) {
                        return Err(format!(
                            "Missing parameter index ?{} in statement (indices must be continuous).",
                            need
                        ));
                    }
                }
                if params_len != *max {
                    return Err(format!(
                        "Expected {} parameters but got {}.",
                        max, params_len
                    ));
                }
                Ok(())
            }
        }
    }

    fn build_param_map(
        &self,
        stmt: *mut sqlite3_stmt,
        mode: &PlaceholderMode,
    ) -> Result<Vec<usize>, String> {
        let param_count = unsafe { sqlite3_bind_parameter_count(stmt) } as usize;
        let mut param_map: Vec<usize> = Vec::with_capacity(param_count);
        for i in 1..=param_count as i32 {
            let name_ptr = unsafe { sqlite3_bind_parameter_name(stmt, i) };
            let target_index = if name_ptr.is_null() {
                match mode {
                    PlaceholderMode::Plain { .. } => (i as usize) - 1,
                    PlaceholderMode::Numbered { .. } => {
                        unreachable!("numbered placeholders never return null names")
                    }
                }
            } else {
                let name = unsafe { CStr::from_ptr(name_ptr) }.to_string_lossy();
                let s = name.as_ref();
                if let Some(stripped) = s.strip_prefix('?') {
                    let n: usize = stripped
                        .parse()
                        .map_err(|_| format!("Invalid parameter index: {}", s))?;
                    if n == 0 {
                        return Err(
                            "Invalid parameter index: ?0 or negative indices are not allowed."
                                .to_string(),
                        );
                    }
                    n - 1
                } else {
                    return Err("Named parameters not supported.".to_string());
                }
            };
            param_map.push(target_index);
        }
        Ok(param_map)
    }

    fn parse_number_param(idx0: usize, num: &serde_json::Number) -> Result<ParamKind, String> {
        if let Some(v) = num.as_i64() {
            Ok(ParamKind::I64(v))
        } else if let Some(v) = num.as_f64() {
            Ok(ParamKind::F64(v))
        } else {
            Err(format!("Unsupported numeric value at index {}", idx0 + 1))
        }
    }

    fn parse_string_param(idx0: usize, s: &str) -> Result<ParamKind, String> {
        let c = CString::new(s.to_string())
            .map_err(|_| format!("String contains NUL at index {}", idx0 + 1))?;
        Ok(ParamKind::Text(c))
    }

    fn parse_object_param(
        idx0: usize,
        map: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<ParamKind, String> {
        let t = map
            .get("__type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("Invalid extended param at index {}", idx0 + 1))?;
        match t {
            "blob" => {
                let b64 = map
                    .get("base64")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| format!("Invalid blob parameter at index {}", idx0 + 1))?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|_| format!("Invalid base64 for blob at index {}", idx0 + 1))?;
                Ok(ParamKind::Blob(bytes))
            }
            "bigint" => {
                let s = map
                    .get("value")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| format!("Invalid bigint parameter at index {}", idx0 + 1))?;
                let v: i64 = s
                    .parse()
                    .map_err(|_| format!("BigInt out of i64 range at index {}.", idx0 + 1))?;
                Ok(ParamKind::I64(v))
            }
            _ => Err(format!(
                "Unsupported extended param type '{}' at index {}",
                t,
                idx0 + 1
            )),
        }
    }

    fn parse_json_param(&self, idx0: usize, v: &serde_json::Value) -> Result<ParamKind, String> {
        Ok(match v {
            serde_json::Value::Null => ParamKind::Null,
            serde_json::Value::Bool(b) => ParamKind::Bool(*b),
            serde_json::Value::Number(num) => Self::parse_number_param(idx0, num)?,
            serde_json::Value::String(s) => Self::parse_string_param(idx0, s)?,
            serde_json::Value::Object(map) => Self::parse_object_param(idx0, map)?,
            _ => return Err(format!("Unsupported parameter value at index {}", idx0 + 1)),
        })
    }

    fn bind_null(&self, stmt: *mut sqlite3_stmt, i: i32) -> Result<(), String> {
        let rc = unsafe { sqlite3_bind_null(stmt, i) };
        if rc != SQLITE_OK {
            let msg = self.sqlite_errmsg();
            return Err(format!("Failed to bind NULL at {i}: {msg}"));
        }
        Ok(())
    }

    fn bind_bool(&self, stmt: *mut sqlite3_stmt, i: i32, b: bool) -> Result<(), String> {
        let rc = unsafe { sqlite3_bind_int(stmt, i, if b { 1 } else { 0 }) };
        if rc != SQLITE_OK {
            let msg = self.sqlite_errmsg();
            return Err(format!("Failed to bind boolean at {i}: {msg}"));
        }
        Ok(())
    }

    fn bind_i64(&self, stmt: *mut sqlite3_stmt, i: i32, v: i64) -> Result<(), String> {
        let rc = unsafe { sqlite3_bind_int64(stmt, i, v) };
        if rc != SQLITE_OK {
            let msg = self.sqlite_errmsg();
            return Err(format!("Failed to bind int64 at {i}: {msg}"));
        }
        Ok(())
    }

    fn bind_f64(&self, stmt: *mut sqlite3_stmt, i: i32, v: f64) -> Result<(), String> {
        let rc = unsafe { sqlite3_bind_double(stmt, i, v) };
        if rc != SQLITE_OK {
            let msg = self.sqlite_errmsg();
            return Err(format!("Failed to bind double at {i}: {msg}"));
        }
        Ok(())
    }

    fn bind_text(
        &self,
        stmt: *mut sqlite3_stmt,
        i: i32,
        c: &CString,
        buffers: &mut BoundBuffers,
    ) -> Result<(), String> {
        buffers._texts.push(c.clone());
        let last = buffers._texts.last().unwrap();
        let ptr = last.as_ptr();
        let len = last.as_bytes().len() as i32;
        let rc = unsafe {
            sqlite3_bind_text(stmt, i, ptr, len, None::<unsafe extern "C" fn(*mut c_void)>)
        };
        if rc != SQLITE_OK {
            let msg = self.sqlite_errmsg();
            return Err(format!("Failed to bind text at {i}: {msg}"));
        }
        Ok(())
    }

    fn bind_blob(
        &self,
        stmt: *mut sqlite3_stmt,
        i: i32,
        bytes: &[u8],
        buffers: &mut BoundBuffers,
    ) -> Result<(), String> {
        buffers._blobs.push(bytes.to_owned());
        let last = buffers._blobs.last().unwrap();
        let buf_ptr = last.as_ptr() as *const _;
        let len = last.len() as i32;
        let rc = unsafe {
            sqlite3_bind_blob(
                stmt,
                i,
                buf_ptr,
                len,
                None::<unsafe extern "C" fn(*mut c_void)>,
            )
        };
        if rc != SQLITE_OK {
            let msg = self.sqlite_errmsg();
            return Err(format!("Failed to bind blob at {i}: {msg}"));
        }
        Ok(())
    }

    fn bind_param(
        &self,
        stmt: *mut sqlite3_stmt,
        i: i32,
        kind: &ParamKind,
        buffers: &mut BoundBuffers,
    ) -> Result<(), String> {
        match kind {
            ParamKind::Null => self.bind_null(stmt, i),
            ParamKind::Bool(b) => self.bind_bool(stmt, i, *b),
            ParamKind::I64(v) => self.bind_i64(stmt, i, *v),
            ParamKind::F64(v) => self.bind_f64(stmt, i, *v),
            ParamKind::Text(c) => self.bind_text(stmt, i, c, buffers),
            ParamKind::Blob(bytes) => self.bind_blob(stmt, i, bytes, buffers),
        }
    }
    fn is_trivia_tail_only(tail: *const i8) -> bool {
        if tail.is_null() {
            return true;
        }
        // Safe because we created the input as a NUL-terminated CString and SQLite returns a pointer into it
        let rest_c = unsafe { CStr::from_ptr(tail) };
        let rest = rest_c.to_bytes();

        // Simple scanner: skip whitespace and comments only
        let mut i = 0usize;
        while i < rest.len() {
            match rest[i] {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    i += 1;
                }
                b'-' => {
                    if i + 1 < rest.len() && rest[i + 1] == b'-' {
                        // line comment -- ... until newline
                        i += 2;
                        while i < rest.len() && rest[i] != b'\n' {
                            i += 1;
                        }
                    } else {
                        return false;
                    }
                }
                b'/' => {
                    if i + 1 < rest.len() && rest[i + 1] == b'*' {
                        // block comment /* ... */
                        i += 2;
                        while i + 1 < rest.len() && !(rest[i] == b'*' && rest[i + 1] == b'/') {
                            i += 1;
                        }
                        if i + 1 < rest.len() {
                            i += 2; // consume */
                        } else {
                            // unterminated comment: treat as non-trivia to be safe
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        true
    }

    fn bind_params_for_stmt(
        &self,
        stmt: *mut sqlite3_stmt,
        params: &[serde_json::Value],
    ) -> Result<BoundBuffers, String> {
        // Derive placeholder mode, validate with provided params, and build mapping
        let mode = self.detect_placeholder_mode(stmt)?;
        self.validate_params_against_mode(&mode, params.len())?;
        let param_map = self.build_param_map(stmt, &mode)?;

        // Keep owned buffers alive for text/blob while the statement executes
        let mut buffers = BoundBuffers {
            _texts: Vec::new(),
            _blobs: Vec::new(),
        };

        let param_count = unsafe { sqlite3_bind_parameter_count(stmt) } as usize;
        for i in 1..=param_count as i32 {
            let target_index = param_map[(i - 1) as usize];
            let val = params.get(target_index).ok_or_else(|| {
                format!(
                    "Missing parameter value at index {} (0-based {})",
                    target_index + 1,
                    target_index
                )
            })?;

            let kind = self.parse_json_param(target_index, val)?;
            self.bind_param(stmt, i, &kind, &mut buffers)?;
        }

        Ok(buffers)
    }

    async fn exec_single_statement_with_params(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<(Option<Vec<serde_json::Value>>, i32), String> {
        let sql_cstr = CString::new(sql).map_err(|e| format!("Invalid SQL string: {e}"))?;
        let ptr = sql_cstr.as_ptr();

        let (stmt_opt, tail) = self.prepare_one(ptr)?;

        if let Some(stmt) = stmt_opt {
            let mut stmt_guard = StmtGuard::new(stmt);

            if !Self::is_trivia_tail_only(tail) {
                return Err("Parameterized queries must contain a single statement.".to_string());
            }

            let param_count = unsafe { sqlite3_bind_parameter_count(stmt) } as usize;
            if param_count == 0 {
                if !params.is_empty() {
                    return Err(format!(
                        "No parameters expected but {} provided.",
                        params.len()
                    ));
                }
                return self.exec_prepared_statement(stmt_guard.take());
            }

            let _buffers_guard = self.bind_params_for_stmt(stmt, &params)?;
            self.exec_prepared_statement(stmt_guard.take())
        } else if Self::is_trivia_tail_only(tail) {
            if !params.is_empty() {
                return Err(format!(
                    "No parameters expected but {} provided.",
                    params.len()
                ));
            }
            Ok((None, 0))
        } else {
            Err("Parameterized queries must contain a single statement.".to_string())
        }
    }
    pub async fn initialize_opfs(db_name: &str) -> Result<Self, JsValue> {
        // Install OPFS VFS and set as default
        install_opfs_sahpool(None, true)
            .await
            .map_err(|e| JsValue::from_str(&format!("Failed to install OPFS VFS: {e:?}")))?;

        // Open database with OPFS
        let mut db: *mut sqlite3 = std::ptr::null_mut();
        let sanitized = sanitize_db_filename(db_name);
        let open_uri = format!("opfs-sahpool:{}", sanitized);
        let db_name = CString::new(open_uri.clone()).map_err(|e| {
            JsValue::from_str(&format!(
                "Invalid database URI (NUL found): {open_uri} ({e})"
            ))
        })?;

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
                if db.is_null() {
                    format!("SQLite open error code: {ret}")
                } else {
                    let ptr = sqlite3_errmsg(db);
                    if !ptr.is_null() {
                        CStr::from_ptr(ptr).to_string_lossy().into_owned()
                    } else {
                        format!("SQLite open error code: {ret}")
                    }
                }
            };
            if !db.is_null() {
                unsafe { sqlite3_close(db) };
            }
            return Err(JsValue::from_str(&format!(
                "Failed to open SQLite database: {error_msg}"
            )));
        }

        // Register custom functions; close DB on failure to avoid leaks
        if let Err(e) = register_custom_functions(db) {
            unsafe { sqlite3_close(db) };
            return Err(JsValue::from_str(&e));
        }

        Ok(SQLiteDatabase {
            db,
            in_transaction: false,
        })
    }

    /// Execute a prepared statement, collecting any result rows and the affected row count.
    /// Returns Some(rows) for queries (column count > 0), even if zero rows; None otherwise.
    fn exec_prepared_statement(
        &self,
        stmt: *mut sqlite3_stmt,
    ) -> Result<(Option<Vec<serde_json::Value>>, i32), String> {
        let guard = StmtGuard::new(stmt);
        let stmt = guard.stmt;

        let col_count = unsafe { sqlite3_column_count(stmt) };
        let is_query = col_count > 0;

        let mut results = Vec::new();
        let mut column_names: Option<Vec<String>> = None;

        loop {
            let step_result = unsafe { sqlite3_step(stmt) };
            match step_result {
                SQLITE_ROW => {
                    if column_names.is_none() {
                        column_names = Some(Self::collect_column_names(stmt));
                    }
                    let names = column_names.as_ref().unwrap();
                    let mut row_obj = std::collections::BTreeMap::new();
                    for i in 0..col_count {
                        let value = Self::read_column_value(stmt, i);
                        if let Some(col_name) = names.get(i as usize) {
                            row_obj.insert(col_name.clone(), value);
                        }
                    }
                    results.push(serde_json::Value::Object(row_obj.into_iter().collect()));
                }
                SQLITE_DONE => break,
                other => {
                    return Err(format!("Query execution failed: {}", self.sqlite_errmsg())
                        .replace(
                            "Unknown SQLite error",
                            &format!("SQLite error code: {other}"),
                        ));
                }
            }
        }

        let changes = unsafe { sqlite3_changes(self.db) };
        if is_query {
            Ok((Some(results), changes))
        } else {
            Ok((None, changes))
        }
    }

    /// Execute a single SQL statement and return the result
    async fn exec_single_statement(
        &self,
        sql: &str,
    ) -> Result<(Option<Vec<serde_json::Value>>, i32), String> {
        let sql_cstr = CString::new(sql).map_err(|e| format!("Invalid SQL string: {e}"))?;
        let mut ptr = sql_cstr.as_ptr();

        loop {
            let (stmt_opt, tail) = self.prepare_one(ptr)?;

            if let Some(stmt) = stmt_opt {
                return self.exec_prepared_statement(stmt);
            }

            if tail.is_null() || tail == ptr {
                return Ok((None, 0));
            }
            ptr = tail;
        }
    }

    /// Execute potentially multiple SQL statements
    pub async fn exec(&mut self, sql: &str) -> Result<String, String> {
        let trimmed = sql.trim();

        // Single-statement mode: execute only the first statement, ignore tail
        if !trimmed.ends_with(';') {
            let (results, affected) = self.exec_single_statement(trimmed).await?;

            self.refresh_transaction_state();

            return if let Some(results) = results {
                serde_json::to_string_pretty(&results)
                    .map_err(|e| format!("JSON serialization error: {e}"))
            } else {
                Ok(format!(
                    "Query executed successfully. Rows affected: {affected}"
                ))
            };
        }

        // Multi-statement mode: use SQLite parser with tail pointer
        let sql_cstr = CString::new(sql).map_err(|e| format!("Invalid SQL string: {e}"))?;
        let mut ptr = sql_cstr.as_ptr();

        let mut select_results: Option<Vec<serde_json::Value>> = None;
        let mut total_affected_rows = 0;
        let mut stmt_index: usize = 0;
        let mut executed_any = false;

        loop {
            let (stmt_opt, tail) = match self.prepare_one(ptr) {
                Ok(v) => v,
                Err(err_msg) => {
                    self.rollback_if_in_transaction().await;
                    return Err(format!("Statement {} failed: {}", stmt_index + 1, err_msg));
                }
            };

            if stmt_opt.is_none() {
                // No statement at this position; advance or finish
                if tail.is_null() || tail == ptr {
                    break;
                } else {
                    ptr = tail;
                    continue;
                }
            }

            // We have a valid statement; execute it
            stmt_index += 1;
            executed_any = true;
            match self.exec_prepared_statement(stmt_opt.unwrap()) {
                Ok((rows_opt, affected)) => {
                    if rows_opt.is_some() && select_results.is_none() {
                        select_results = rows_opt;
                    }
                    total_affected_rows += affected;
                }
                Err(err) => {
                    self.rollback_if_in_transaction().await;
                    return Err(format!("Statement {} failed: {}", stmt_index, err));
                }
            }

            // Advance to the tail of this statement
            if tail.is_null() || tail == ptr {
                break;
            } else {
                ptr = tail;
            }
        }

        self.refresh_transaction_state();

        if !executed_any {
            return Ok("No statements to execute.".to_string());
        }

        if let Some(results) = select_results {
            serde_json::to_string_pretty(&results)
                .map_err(|e| format!("JSON serialization error: {e}"))
        } else {
            Ok(format!(
                "Query executed successfully. Rows affected: {total_affected_rows}"
            ))
        }
    }

    /// Execute a single parameterized SQL statement with binding and return the result
    pub async fn exec_with_params(
        &mut self,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<String, String> {
        let (results, affected) = self.exec_single_statement_with_params(sql, params).await?;

        self.refresh_transaction_state();

        if let Some(results) = results {
            serde_json::to_string_pretty(&results)
                .map_err(|e| format!("JSON serialization error: {e}"))
        } else {
            Ok(format!(
                "Query executed successfully. Rows affected: {affected}"
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

#[cfg(all(test, target_family = "wasm"))]
mod tests {
    use super::*;
    use serde_json::json;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    async fn test_opfs_initialization_success() {
        let result = SQLiteDatabase::initialize_opfs("testdb").await;
        if result.is_err() {
            return;
        }
        assert!(
            result.is_ok(),
            "OPFS database initialization should succeed"
        );
        let db = result.unwrap();
        assert!(
            !db.db.is_null(),
            "Database pointer should not be null after initialization"
        );
    }

    async fn get_test_db() -> Option<SQLiteDatabase> {
        (SQLiteDatabase::initialize_opfs("testdb").await).ok()
    }

    #[wasm_bindgen_test]
    async fn test_create_table_and_insert() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        let create_result = db
            .exec("CREATE TABLE test_users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)")
            .await;
        assert!(
            create_result.is_ok(),
            "CREATE TABLE should execute successfully"
        );
        assert!(
            create_result.unwrap().contains("Rows affected: 0"),
            "CREATE TABLE should report 0 rows affected"
        );

        let insert_result = db
            .exec("INSERT INTO test_users (name, age) VALUES ('Alice', 25)")
            .await;
        assert!(
            insert_result.is_ok(),
            "INSERT statement should execute successfully"
        );
        assert!(
            insert_result.unwrap().contains("Rows affected: 1"),
            "INSERT should report 1 row affected"
        );
    }

    #[wasm_bindgen_test]
    async fn test_select_query_with_results() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE test_products (id INTEGER PRIMARY KEY, name TEXT, price REAL)")
            .await
            .expect("Create failed");
        db.exec("INSERT INTO test_products (name, price) VALUES ('Laptop', 999.99)")
            .await
            .expect("Insert failed");
        db.exec("INSERT INTO test_products (name, price) VALUES ('Mouse', 25.50)")
            .await
            .expect("Insert failed");

        let result = db.exec("SELECT * FROM test_products ORDER BY id").await;
        assert!(result.is_ok(), "SELECT query should execute successfully");

        let json_str = result.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        assert_eq!(array.len(), 2, "Should return exactly 2 rows");

        let first = &array[0];
        assert_eq!(
            first["name"].as_str().unwrap(),
            "Laptop",
            "First product name should be 'Laptop'"
        );
        assert_eq!(
            first["price"].as_f64().unwrap(),
            999.99,
            "First product price should be 999.99"
        );

        let second = &array[1];
        assert_eq!(
            second["name"].as_str().unwrap(),
            "Mouse",
            "Second product name should be 'Mouse'"
        );
        assert_eq!(
            second["price"].as_f64().unwrap(),
            25.50,
            "Second product price should be 25.50"
        );
    }

    #[wasm_bindgen_test]
    async fn test_select_empty_result() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE empty_table (id INTEGER)")
            .await
            .expect("Create failed");

        let result = db.exec("SELECT * FROM empty_table").await;
        assert!(
            result.is_ok(),
            "SELECT from empty table should execute successfully"
        );
        assert_eq!(
            result.unwrap(),
            "[]",
            "Empty SELECT should return empty JSON array"
        );
    }

    #[wasm_bindgen_test]
    async fn test_integer_column_handling() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE test_ints (small_int INTEGER, big_int INTEGER)")
            .await
            .expect("Create failed");
        db.exec("INSERT INTO test_ints VALUES (42, 9223372036854775807)")
            .await
            .expect("Insert failed");

        let result = db
            .exec("SELECT * FROM test_ints")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        let row = &array[0];

        assert_eq!(
            row["small_int"].as_i64().unwrap(),
            42,
            "Small integer should be 42"
        );
        assert_eq!(
            row["big_int"].as_i64().unwrap(),
            9223372036854775807,
            "Large integer should be max i64 value"
        );
    }

    #[wasm_bindgen_test]
    async fn test_float_column_handling() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE test_floats (pi REAL, negative REAL)")
            .await
            .expect("Create failed");
        db.exec("INSERT INTO test_floats VALUES (3.14159, -2.71828)")
            .await
            .expect("Insert failed");

        let result = db
            .exec("SELECT * FROM test_floats")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        let row = &array[0];

        assert!(
            (row["pi"].as_f64().unwrap() - std::f64::consts::PI).abs() < 0.00001,
            "Pi should be approximately 3.14159"
        );
        assert!(
            (row["negative"].as_f64().unwrap() - (-std::f64::consts::E)).abs() < 0.00001,
            "Negative float should be approximately -2.71828"
        );
    }

    #[wasm_bindgen_test]
    async fn test_text_column_handling() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE test_text (message TEXT, empty TEXT, null_val TEXT)")
            .await
            .expect("Create failed");
        db.exec("INSERT INTO test_text VALUES ('Hello World', '', NULL)")
            .await
            .expect("Insert failed");

        let result = db
            .exec("SELECT * FROM test_text")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        let row = &array[0];

        assert_eq!(
            row["message"].as_str().unwrap(),
            "Hello World",
            "Text column should contain 'Hello World'"
        );
        assert_eq!(
            row["empty"].as_str().unwrap(),
            "",
            "Empty text column should be empty string"
        );
        assert!(
            row["null_val"].is_null(),
            "NULL text column should be null in JSON"
        );
    }

    // exec_with_params integration tests
    // 1) Positional '?' bindings with multiple types
    #[wasm_bindgen_test]
    async fn test_exec_with_params_positional_multiple_types() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec(
            "CREATE TABLE params_positional (
                null_col TEXT,
                bool_col INTEGER,
                int_col INTEGER,
                float_col REAL,
                text_col TEXT
            )",
        )
        .await
        .expect("Create failed");

        // Insert with plain positional placeholders
        let insert_res = db
            .exec_with_params(
                "INSERT INTO params_positional (null_col, bool_col, int_col, float_col, text_col) VALUES (?, ?, ?, ?, ?)",
                vec![json!(null), json!(true), json!(42), json!(3.5), json!("hello")],
            )
            .await;
        assert!(insert_res.is_ok(), "INSERT with params should succeed");
        assert!(
            insert_res.unwrap().contains("Rows affected: 1"),
            "INSERT should report 1 row affected"
        );

        // Select using positional binding as well
        let select_json = db
            .exec_with_params(
                "SELECT null_col, bool_col, int_col, float_col, text_col FROM params_positional WHERE int_col = ?",
                vec![json!(42)],
            )
            .await
            .expect("SELECT with params failed");
        let parsed: serde_json::Value = serde_json::from_str(&select_json).expect("Invalid JSON");
        let rows = parsed.as_array().expect("Expected array JSON");
        assert_eq!(rows.len(), 1, "Should return one matching row");
        let row = &rows[0];
        assert!(row["null_col"].is_null(), "Null round-trips as null");
        assert_eq!(row["bool_col"].as_i64().unwrap(), 1, "true -> 1");
        assert_eq!(row["int_col"].as_i64().unwrap(), 42);
        assert!((row["float_col"].as_f64().unwrap() - 3.5).abs() < 1e-9);
        assert_eq!(row["text_col"].as_str().unwrap(), "hello");
    }

    // 2) Numbered placeholders '?N' including gap detection and duplicates
    #[wasm_bindgen_test]
    async fn test_exec_with_params_numbered_duplicate_indices_allowed() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE numbered_dup (a INTEGER, b INTEGER)")
            .await
            .expect("Create failed");

        // Duplicate ?1 should reuse the same bound value
        let res = db
            .exec_with_params(
                "INSERT INTO numbered_dup (a, b) VALUES (?1, ?1)",
                vec![json!(7)],
            )
            .await;
        assert!(res.is_ok(), "Duplicate numbered index should succeed");

        let out = db
            .exec("SELECT a, b FROM numbered_dup")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("Invalid JSON");
        let rows = parsed.as_array().expect("Expected array JSON");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["a"].as_i64().unwrap(), 7);
        assert_eq!(rows[0]["b"].as_i64().unwrap(), 7);
    }

    #[wasm_bindgen_test]
    async fn test_exec_with_params_numbered_gap_error() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        // Use ?1 and ?3 with a missing ?2 to trigger continuity error
        let err = db
            .exec_with_params("SELECT ?1, ?3", vec![json!(10), json!(30)])
            .await
            .unwrap_err();
        assert!(
            err.contains("Missing parameter index ?2"),
            "Should report missing index in numbered placeholders"
        );
    }

    // 3) BLOB object and bigint-as-string handling
    #[wasm_bindgen_test]
    async fn test_exec_with_params_blob_and_bigint() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE binint_test (b BLOB, bi INTEGER)")
            .await
            .expect("Create failed");

        // Base64 for bytes: "Rust" -> 52 75 73 74
        let blob_b64 = base64::engine::general_purpose::STANDARD.encode(b"Rust");
        let big_str = "9223372036854775807"; // i64::MAX

        let res = db
            .exec_with_params(
                "INSERT INTO binint_test (b, bi) VALUES (?, ?)",
                vec![
                    json!({"__type":"blob","base64": blob_b64}),
                    json!({"__type":"bigint","value": big_str}),
                ],
            )
            .await;
        assert!(res.is_ok(), "INSERT blob/bigint should succeed");

        // Verify using a SELECT that returns numeric length and bigint value
        let verify = db
            .exec("SELECT length(b) AS blen, bi FROM binint_test")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&verify).expect("Invalid JSON");
        let rows = parsed.as_array().expect("Expected array JSON");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["blen"].as_i64().unwrap(), 4, "Blob length matches");
        assert_eq!(
            rows[0]["bi"].as_i64().unwrap(),
            9_223_372_036_854_775_807,
            "Bigint stored and returned as i64"
        );

        // Also check the blob pretty string form when selecting the BLOB directly
        let blob_str = db
            .exec("SELECT b FROM binint_test")
            .await
            .expect("Select blob failed");
        let blob_val: serde_json::Value =
            serde_json::from_str(&blob_str).expect("Invalid JSON for blob row");
        let arr = blob_val.as_array().expect("Expected array JSON");
        let bstr = arr[0]["b"]
            .as_str()
            .expect("Expected string marker for blob");
        assert!(
            bstr.contains("<blob 4 bytes>"),
            "Blob marker includes length"
        );
    }

    #[wasm_bindgen_test]
    async fn test_blob_column_handling() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE test_blob (data BLOB)")
            .await
            .expect("Create failed");
        db.exec("INSERT INTO test_blob VALUES (X'48656C6C6F')")
            .await
            .expect("Insert failed");

        let result = db
            .exec("SELECT * FROM test_blob")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        let row = &array[0];

        let blob_str = row["data"].as_str().unwrap();
        assert!(
            blob_str.starts_with("<blob"),
            "BLOB data should start with '<blob'"
        );
        assert!(
            blob_str.contains("bytes>"),
            "BLOB data should contain 'bytes>''"
        );
    }

    #[wasm_bindgen_test]
    async fn test_column_names_handling() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE test_cols (id INTEGER, full_name TEXT, \"quoted col\" INTEGER)")
            .await
            .expect("Create failed");
        db.exec("INSERT INTO test_cols VALUES (1, 'John Doe', 100)")
            .await
            .expect("Insert failed");

        let result = db
            .exec("SELECT * FROM test_cols")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        let row = &array[0];

        assert!(row.get("id").is_some(), "Should have 'id' column");
        assert!(
            row.get("full_name").is_some(),
            "Should have 'full_name' column"
        );
        assert!(
            row.get("quoted col").is_some(),
            "Should have 'quoted col' column with spaces"
        );
        assert_eq!(
            row["full_name"].as_str().unwrap(),
            "John Doe",
            "full_name should be 'John Doe'"
        );
    }

    #[wasm_bindgen_test]
    async fn test_update_query() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE test_update (id INTEGER, value INTEGER)")
            .await
            .expect("Create failed");
        db.exec("INSERT INTO test_update VALUES (1, 10), (2, 20), (3, 30)")
            .await
            .expect("Insert failed");

        let result = db
            .exec("UPDATE test_update SET value = value * 2 WHERE id > 1")
            .await;
        assert!(result.is_ok());
        let update_result = result.unwrap();
        assert!(
            update_result.contains("Rows affected: 2"),
            "UPDATE should affect exactly 2 rows"
        );

        let select_result = db
            .exec("SELECT value FROM test_update ORDER BY id")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&select_result).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");

        assert_eq!(
            array[0]["value"].as_i64().unwrap(),
            10,
            "First row value should remain 10"
        );
        assert_eq!(
            array[1]["value"].as_i64().unwrap(),
            40,
            "Second row value should be doubled to 40"
        );
        assert_eq!(
            array[2]["value"].as_i64().unwrap(),
            60,
            "Third row value should be doubled to 60"
        );
    }

    #[wasm_bindgen_test]
    async fn test_delete_query() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE test_delete (id INTEGER, name TEXT)")
            .await
            .expect("Create failed");
        db.exec("INSERT INTO test_delete VALUES (1, 'keep'), (2, 'delete'), (3, 'delete')")
            .await
            .expect("Insert failed");

        let result = db
            .exec("DELETE FROM test_delete WHERE name = 'delete'")
            .await;
        assert!(result.is_ok());
        let delete_result = result.unwrap();
        assert!(
            delete_result.contains("Rows affected: 2"),
            "DELETE should affect exactly 2 rows"
        );

        let select_result = db
            .exec("SELECT COUNT(*) as count FROM test_delete")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&select_result).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        assert_eq!(
            array[0]["count"].as_i64().unwrap(),
            1,
            "Should have exactly 1 row remaining after delete"
        );
    }

    #[wasm_bindgen_test]
    async fn test_invalid_sql_syntax_error() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        let result = db.exec("INVALID SQL SYNTAX HERE").await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            error.contains("Failed to prepare statement"),
            "Invalid SQL should produce prepare statement error"
        );
    }

    #[wasm_bindgen_test]
    async fn test_sql_with_null_byte_error() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        let result = db.exec("SELECT * FROM table\0WITH NULL").await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            error.contains("Invalid SQL string"),
            "SQL with null bytes should produce invalid string error"
        );
    }

    #[wasm_bindgen_test]
    async fn test_table_not_found_error() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        let result = db.exec("SELECT * FROM nonexistent_table").await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            error.contains("Query execution failed") || error.contains("no such table"),
            "Query on nonexistent table should fail with appropriate error"
        );
    }

    #[wasm_bindgen_test]
    async fn test_custom_functions_available() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        let result = db.exec("SELECT float_add('1.5', '2.5') as result").await;
        assert!(result.is_ok());

        let json_str = result.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        let row = &array[0];

        assert!(
            row.get("result").is_some(),
            "Custom function should return a result"
        );
    }

    #[wasm_bindgen_test]
    async fn test_database_drop_cleanup() {
        {
            let Some(db) = get_test_db().await else {
                return;
            };
            assert!(
                !db.db.is_null(),
                "Database pointer should be valid before drop"
            );
        }
    }

    #[wasm_bindgen_test]
    async fn test_multiple_statements_handling() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        let result = db
            .exec("CREATE TABLE multi_test (id INTEGER); INSERT INTO multi_test VALUES (1);")
            .await;
        assert!(
            result.is_ok(),
            "Multiple statements should execute successfully"
        );
    }

    #[wasm_bindgen_test]
    async fn test_sequential_database_operations() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE sequential_test (id INTEGER PRIMARY KEY, value TEXT)")
            .await
            .expect("Create failed");

        let insert1 = db
            .exec("INSERT INTO sequential_test (value) VALUES ('first')")
            .await;
        let insert2 = db
            .exec("INSERT INTO sequential_test (value) VALUES ('second')")
            .await;

        assert!(insert1.is_ok(), "First INSERT should succeed");
        assert!(insert2.is_ok(), "Second INSERT should succeed");

        let result = db
            .exec("SELECT COUNT(*) as count FROM sequential_test")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        assert_eq!(
            array[0]["count"].as_i64().unwrap(),
            2,
            "Should have exactly 2 rows after sequential inserts"
        );
    }

    #[wasm_bindgen_test]
    async fn test_multi_statement_create_and_insert() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        let result = db
            .exec("CREATE TABLE multi_users (id INTEGER PRIMARY KEY, name TEXT); INSERT INTO multi_users (name) VALUES ('Alice'); INSERT INTO multi_users (name) VALUES ('Bob')")
            .await;

        assert!(
            result.is_ok(),
            "Multi-statement should execute successfully"
        );
        assert!(
            result.unwrap().contains("Rows affected: 2"),
            "Should affect 2 rows total"
        );

        let select_result = db
            .exec("SELECT COUNT(*) as count FROM multi_users")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&select_result).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        assert_eq!(
            array[0]["count"].as_i64().unwrap(),
            2,
            "Should have 2 rows after multi-statement insert"
        );
    }

    #[wasm_bindgen_test]
    async fn test_transaction_with_multi_statement() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE transaction_test (id INTEGER PRIMARY KEY, value INTEGER)")
            .await
            .expect("Create failed");

        let result = db
            .exec("BEGIN TRANSACTION; INSERT INTO transaction_test (value) VALUES (100); INSERT INTO transaction_test (value) VALUES (200); COMMIT")
            .await;

        assert!(result.is_ok(), "Transaction should execute successfully");
        assert!(
            result.unwrap().contains("Rows affected: 2"),
            "Should affect 2 rows in transaction"
        );

        let select_result = db
            .exec("SELECT COUNT(*) as count FROM transaction_test")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&select_result).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        assert_eq!(
            array[0]["count"].as_i64().unwrap(),
            2,
            "Should have 2 rows after successful transaction"
        );
    }

    #[wasm_bindgen_test]
    async fn test_transaction_with_error_auto_rollback() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE rollback_test (id INTEGER PRIMARY KEY, value INTEGER)")
            .await
            .expect("Create failed");

        let result = db
            .exec("BEGIN TRANSACTION; INSERT INTO rollback_test (value) VALUES (300); INSERT INTO nonexistent_table (value) VALUES (400); COMMIT")
            .await;

        assert!(result.is_err(), "Transaction with error should fail");
        assert!(
            result.unwrap_err().contains("Statement 2 failed"),
            "Should indicate which statement failed"
        );

        let select_result = db
            .exec("SELECT COUNT(*) as count FROM rollback_test")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&select_result).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        assert_eq!(
            array[0]["count"].as_i64().unwrap(),
            0,
            "Should have 0 rows after failed transaction (auto-rollback)"
        );
    }

    #[wasm_bindgen_test]
    async fn test_mixed_select_and_modification_statements() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE mixed_test (id INTEGER PRIMARY KEY, name TEXT)")
            .await
            .expect("Create failed");

        let result = db
            .exec("INSERT INTO mixed_test (name) VALUES ('First'); SELECT * FROM mixed_test; INSERT INTO mixed_test (name) VALUES ('Second')")
            .await;

        assert!(
            result.is_ok(),
            "Mixed statements should execute successfully"
        );
        let result_str = result.unwrap();

        let parsed: serde_json::Value =
            serde_json::from_str(&result_str).expect("Should be valid JSON");
        let array = parsed.as_array().expect("Should be array");
        assert_eq!(
            array.len(),
            1,
            "Should return results from SELECT statement"
        );
        assert_eq!(
            array[0]["name"].as_str().unwrap(),
            "First",
            "Should have first inserted value"
        );

        let count_result = db
            .exec("SELECT COUNT(*) as count FROM mixed_test")
            .await
            .expect("Select failed");
        let count_parsed: serde_json::Value =
            serde_json::from_str(&count_result).expect("Invalid JSON");
        let count_array = count_parsed.as_array().expect("Should be array");
        assert_eq!(
            count_array[0]["count"].as_i64().unwrap(),
            2,
            "Both INSERT statements should have been executed"
        );
    }

    #[wasm_bindgen_test]
    async fn test_empty_statements_handling() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        let result = db.exec(";;; ; ").await;

        assert!(result.is_ok(), "Empty statements should not error");
        assert_eq!(
            result.unwrap(),
            "No statements to execute.",
            "Should handle empty input gracefully"
        );
    }

    #[wasm_bindgen_test]
    async fn test_transaction_state_tracking() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE state_test (id INTEGER)")
            .await
            .expect("Create failed");

        assert!(!db.in_transaction, "Should not be in transaction initially");

        db.exec("BEGIN TRANSACTION").await.expect("Begin failed");

        assert!(db.in_transaction, "Should be in transaction after BEGIN");

        db.exec("COMMIT").await.expect("Commit failed");

        assert!(
            !db.in_transaction,
            "Should not be in transaction after COMMIT"
        );
    }

    #[wasm_bindgen_test]
    async fn test_sql_splitting_utility() {
        // Ensure production logic handles multiple semicolons and empty statements gracefully
        let Some(mut db) = get_test_db().await else {
            return;
        };

        // Multiple statements with empty fragments should execute the non-empty ones
        let res = db
            .exec("; SELECT 1; ; SELECT 2; ;")
            .await
            .expect("Execution failed");
        let parsed: serde_json::Value = serde_json::from_str(&res).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        assert_eq!(array.len(), 1, "Only first result set should be returned");
    }

    #[wasm_bindgen_test]
    async fn test_sql_injection_prevention() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE injection_test (id INTEGER PRIMARY KEY, username TEXT)")
            .await
            .expect("Create failed");

        db.exec("INSERT INTO injection_test (username) VALUES ('alice')")
            .await
            .expect("Insert failed");

        let injection_attempt = "SELECT * FROM injection_test WHERE username = ''; DELETE FROM injection_test WHERE 1=1";
        let result = db.exec(injection_attempt).await;

        assert!(result.is_ok(), "Query should execute without error");

        let count_result = db
            .exec("SELECT COUNT(*) as count FROM injection_test")
            .await
            .expect("Count query failed");
        let parsed: serde_json::Value = serde_json::from_str(&count_result).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        assert_eq!(
            array[0]["count"].as_i64().unwrap(),
            1,
            "Data should not be deleted by injection attempt (no trailing semicolon)"
        );

        let multi_statement_with_semicolon = "INSERT INTO injection_test (username) VALUES ('bob'); DELETE FROM injection_test WHERE username = 'bob';";
        let result2 = db.exec(multi_statement_with_semicolon).await;

        assert!(
            result2.is_ok(),
            "Multi-statement with trailing semicolon should work"
        );

        let final_count = db
            .exec("SELECT COUNT(*) as count FROM injection_test")
            .await
            .expect("Final count failed");
        let final_parsed: serde_json::Value =
            serde_json::from_str(&final_count).expect("Invalid JSON");
        let final_array = final_parsed.as_array().expect("Should be array");
        assert_eq!(
            final_array[0]["count"].as_i64().unwrap(),
            1,
            "Multi-statement with trailing semicolon should execute both insert and delete"
        );
    }

    #[wasm_bindgen_test]
    async fn test_semicolon_in_string_literal_multi_statement() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE semi_string_test (name TEXT)")
            .await
            .expect("Create failed");

        // Semicolon inside the string literal should NOT split the statement.
        let sql = "INSERT INTO semi_string_test (name) VALUES ('a; b'); INSERT INTO semi_string_test (name) VALUES ('c');";
        let result = db.exec(sql).await;
        assert!(
            result.is_ok(),
            "Statements with ';' in strings should execute"
        );

        let rows = db
            .exec("SELECT name FROM semi_string_test ORDER BY rowid")
            .await
            .expect("Select failed");
        let parsed: serde_json::Value = serde_json::from_str(&rows).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        assert_eq!(array.len(), 2, "Should have inserted two rows");
        assert_eq!(array[0]["name"].as_str().unwrap(), "a; b");
        assert_eq!(array[1]["name"].as_str().unwrap(), "c");
    }

    #[wasm_bindgen_test]
    async fn test_semicolons_in_comments_do_not_split() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        db.exec("CREATE TABLE comment_split_test (id INTEGER)")
            .await
            .expect("Create failed");

        // Both block and line comments contain semicolons; they should not split statements.
        let sql = "/* leading; comment; with; semicolons */ INSERT INTO comment_split_test (id) VALUES (1); -- trailing; comment;\nINSERT INTO comment_split_test (id) VALUES (2);";
        let result = db.exec(sql).await;
        assert!(
            result.is_ok(),
            "Statements with ';' in comments should execute"
        );

        let rows = db
            .exec("SELECT COUNT(*) as count FROM comment_split_test")
            .await
            .expect("Count failed");
        let parsed: serde_json::Value = serde_json::from_str(&rows).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        assert_eq!(array[0]["count"].as_i64().unwrap(), 2);
    }

    #[wasm_bindgen_test]
    async fn test_create_trigger_with_semicolons_in_body() {
        let Some(mut db) = get_test_db().await else {
            return;
        };

        // Create source and log tables (multi-statement create)
        db.exec("CREATE TABLE trg_src (id INTEGER); CREATE TABLE trg_log (msg TEXT);")
            .await
            .expect("Create tables failed");

        // Create a trigger whose body contains statements separated by semicolons
        // and also a string literal that itself includes a semicolon.
        let trigger_sql = "CREATE TRIGGER trg_after_insert AFTER INSERT ON trg_src BEGIN INSERT INTO trg_log (msg) VALUES ('insert; happened'); INSERT INTO trg_log (msg) VALUES ('second; line'); END;";
        db.exec(trigger_sql).await.expect("Create trigger failed");

        // Fire the trigger
        db.exec("INSERT INTO trg_src (id) VALUES (123)")
            .await
            .expect("Insert into src failed");

        // Verify trigger body executed both statements and preserved semicolons in literals
        let rows = db
            .exec("SELECT msg FROM trg_log ORDER BY rowid")
            .await
            .expect("Select from log failed");
        let parsed: serde_json::Value = serde_json::from_str(&rows).expect("Invalid JSON");
        let array = parsed.as_array().expect("Should be array");
        assert_eq!(array.len(), 2, "Trigger should have inserted two rows");
        assert_eq!(array[0]["msg"].as_str().unwrap(), "insert; happened");
        assert_eq!(array[1]["msg"].as_str().unwrap(), "second; line");
    }
}
