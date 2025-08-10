// lib.rs - Fully self-contained worker with embedded WASM
use wasm_bindgen::prelude::*;
use web_sys::{Blob, BlobPropertyBag, Url, Worker, MessageEvent};
use js_sys::Array;
use std::cell::RefCell;
use std::rc::Rc;

mod worker;

/// Main database connection - creates worker with embedded WASM
#[wasm_bindgen]
pub struct DatabaseConnection {
    worker: Worker,
    pending_queries: Rc<RefCell<Vec<(js_sys::Function, js_sys::Function)>>>,
}

#[wasm_bindgen]
impl DatabaseConnection {
    /// Create a new database connection with fully embedded worker
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<DatabaseConnection, JsValue> {
        web_sys::console::log_1(&"Creating self-contained worker...".into());
        
        // Create the worker with embedded WASM and glue code
        let worker_code = Self::generate_self_contained_worker();
        
        // Create a Blob with the worker code
        let blob_parts = Array::new();
        blob_parts.push(&JsValue::from_str(&worker_code));
        
        let blob_options = BlobPropertyBag::new();
        blob_options.set_type("application/javascript");
        
        let blob = Blob::new_with_str_sequence_and_options(
            &blob_parts,
            &blob_options
        )?;
        
        // Create a blob URL
        let worker_url = Url::create_object_url_with_blob(&blob)?;
        
        // Create the worker from the blob URL
        let worker = Worker::new(&worker_url)?;
        
        // Clean up the blob URL
        Url::revoke_object_url(&worker_url)?;
        
        let pending_queries: Rc<RefCell<Vec<(js_sys::Function, js_sys::Function)>>> = Rc::new(RefCell::new(Vec::new()));
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
                        if let Ok(error) = js_sys::Reflect::get(&data, &JsValue::from_str("error")) {
                            web_sys::console::error_1(&format!("❌ Worker error: {:?}", error).into());
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
                            if let Ok(error) = js_sys::Reflect::get(&data, &JsValue::from_str("error")) {
                                if !error.is_null() && !error.is_undefined() {
                                    let error_str = error.as_string().unwrap_or_else(|| format!("{:?}", error));
                                    let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(&error_str));
                                    return;
                                }
                            }
                            
                            // Handle successful result
                            if let Ok(result) = js_sys::Reflect::get(&data, &JsValue::from_str("result")) {
                                if !result.is_null() && !result.is_undefined() {
                                    let result_str = result.as_string().unwrap_or_else(|| format!("{:?}", result));
                                    let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str(&result_str));
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
    #[wasm_bindgen]
    pub fn query(&self, sql: &str) -> js_sys::Promise {
        let worker = &self.worker;
        let pending_queries = Rc::clone(&self.pending_queries);
        let sql = sql.to_string();
        
        js_sys::Promise::new(&mut |resolve, reject| {
            // Store the promise callbacks
            pending_queries.borrow_mut().push((resolve, reject));
            
            // Send query to worker - create JavaScript object directly
            let message = js_sys::Object::new();
            js_sys::Reflect::set(&message, &JsValue::from_str("type"), &JsValue::from_str("execute-query")).unwrap();
            js_sys::Reflect::set(&message, &JsValue::from_str("sql"), &JsValue::from_str(&sql)).unwrap();
            
            let _ = worker.post_message(&message);
        })
    }
    
    /// Generate worker code that loads WASM dynamically
    fn generate_self_contained_worker() -> String {
        format!(r#"
// ============================================
// Worker that loads WASM dynamically
// ============================================

// Import the WASM module using dynamic import with absolute URL
async function initializeWorker() {{
    try {{
        console.log('[Worker] Loading WASM module...');
        
        // Get the base URL from the current location
        const baseUrl = self.location.origin;
        const wasmModuleUrl = `${{baseUrl}}/pkg/sqlite_worker.js`;
        
        console.log('[Worker] Importing from:', wasmModuleUrl);
        
        // Import the generated WASM module
        const wasmModule = await import(wasmModuleUrl);
        console.log('[Worker] Module loaded, initializing WASM...');
        
        await wasmModule.default();
        
        console.log('[Worker] Calling worker_main...');
        // Call our Rust worker_main function - this initializes the real SQLite worker
        wasmModule.worker_main();
        
        console.log('[Worker] Worker initialized successfully!');
        self.postMessage({{ type: 'worker-ready' }});
        
    }} catch (error) {{
        console.error('[Worker] Initialization failed:', error);
        self.postMessage({{ 
            type: 'worker-error', 
            error: error.toString() 
        }});
    }}
}}

// The Rust worker_main function will set up its own message handler
// We don't need to handle messages here since the Rust code will do it

// Start initialization
initializeWorker();
"#)
    }
    
}

