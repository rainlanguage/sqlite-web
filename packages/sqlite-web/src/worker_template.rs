/// Generate self-contained worker with embedded WASM and JS glue code
/// and inject the database name into the worker global scope so core
/// can read it during initialization.
pub fn generate_self_contained_worker(db_name: &str) -> String {
    // Safely JSON-encode the db name for JS embedding
    let encoded = serde_json::to_string(db_name).unwrap_or_else(|_| "\"unknown\"".to_string());
    let embedded_body = serde_json::to_string(include_str!("embedded_worker.js"))
        .unwrap_or_else(|_| "\"\"".to_string());
    // __SQLITE_EMBEDDED_WORKER stores the JSON-encoded embedded worker body (embedded_body) so the coordinator can spawn a separate DB worker (see coordination.rs:301-313); set when embedded-worker mode is used and consumers must JSON-decode before instantiating the worker.
    let prefix = format!(
        "self.__SQLITE_DB_NAME = {};\nself.__SQLITE_FOLLOWER_TIMEOUT_MS = 5000.0;\nself.__SQLITE_QUERY_TIMEOUT_MS = 30000.0;\nself.__SQLITE_EMBEDDED_WORKER = {};\n",
        encoded, embedded_body
    );
    // Use the bundled worker template with embedded WASM
    let body = include_str!("embedded_worker.js");
    format!("{}{}", prefix, body)
}

#[cfg(all(test, target_family = "wasm"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn embeds_db_name_and_timeout_configuration() {
        let output = generate_self_contained_worker("my_db");
        assert!(
            output.contains("self.__SQLITE_DB_NAME = \"my_db\";"),
            "db name should be JSON encoded in prefix"
        );
        assert!(
            output.contains("self.__SQLITE_FOLLOWER_TIMEOUT_MS = 5000.0;"),
            "timeout constant should be injected"
        );
        assert!(
            output.contains("self.__SQLITE_QUERY_TIMEOUT_MS = 30000.0;"),
            "query timeout constant should be injected"
        );
        assert!(
            output.contains("self.__SQLITE_EMBEDDED_WORKER = "),
            "embedded worker body should be stored on the global"
        );
    }

    #[wasm_bindgen_test]
    fn appends_embedded_worker_body() {
        let output = generate_self_contained_worker("whatever");
        let body = include_str!("embedded_worker.js");
        assert!(
            output.ends_with(body),
            "template output should append embedded worker body verbatim"
        );
    }
}
