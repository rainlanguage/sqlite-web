/// Generate worker code that loads WASM dynamically
pub fn generate_self_contained_worker() -> String {
    format!(
        r#"
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
"#
    )
}
