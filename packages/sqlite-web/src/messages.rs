#[cfg(all(test, target_family = "wasm"))]
pub const WORKER_ERROR_TYPE_GENERIC: &str = "WorkerError";
pub const WORKER_ERROR_TYPE_INITIALIZATION_PENDING: &str = "InitializationPending";
