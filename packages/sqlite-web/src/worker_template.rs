/// Generate self-contained worker with embedded WASM and JS glue code
/// and inject the database name into the worker global scope so core
/// can read it during initialization.
pub fn generate_self_contained_worker(db_name: &str) -> String {
    // Safely JSON-encode the db name for JS embedding
    let encoded = serde_json::to_string(db_name).unwrap_or_else(|_| "\"unknown\"".to_string());
    let prefix = format!(
        "self.__SQLITE_DB_NAME = {};\nself.__SQLITE_FOLLOWER_TIMEOUT_MS = 5000.0;\n",
        encoded
    );
    // Use the bundled worker template with embedded WASM
    let body = include_str!("embedded_worker.js");
    format!("{}{}", prefix, body)
}
