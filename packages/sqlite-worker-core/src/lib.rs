use wasm_bindgen::prelude::*;

mod coordination;
mod database;
mod database_functions;
mod messages;
mod worker;

// Export the worker entry point
#[wasm_bindgen]
pub fn worker_main() {
    console_error_panic_hook::set_once();
    let _ = worker::main();
}

// Re-export modules that might be needed
pub use coordination::*;
pub use database::*;
pub use messages::*;
