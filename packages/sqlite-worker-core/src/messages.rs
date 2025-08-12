use js_sys::Function;
use serde::{Deserialize, Serialize};

// Message types for BroadcastChannel communication
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum ChannelMessage {
    #[serde(rename = "new-leader")]
    NewLeader {
        #[serde(rename = "leaderId")]
        leader_id: String,
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

pub struct PendingQuery {
    pub resolve: Function,
    pub reject: Function,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    // These tests don't require browser environment - just serde functionality
    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn test_channel_message_new_leader_serialization() {
        let msg = ChannelMessage::NewLeader {
            leader_id: "test-leader-123".to_string(),
        };

        let json = serde_json::to_string(&msg).expect("Should serialize");
        assert!(json.contains("\"type\":\"new-leader\""));
        assert!(json.contains("\"leaderId\":\"test-leader-123\""));

        let deserialized: ChannelMessage = serde_json::from_str(&json).expect("Should deserialize");
        match deserialized {
            ChannelMessage::NewLeader { leader_id } => {
                assert_eq!(leader_id, "test-leader-123");
            }
            _ => panic!("Wrong message type deserialized"),
        }
    }

    #[wasm_bindgen_test]
    fn test_channel_message_query_request_serialization() {
        let msg = ChannelMessage::QueryRequest {
            query_id: "query-456".to_string(),
            sql: "SELECT * FROM users".to_string(),
        };

        let json = serde_json::to_string(&msg).expect("Should serialize");
        assert!(json.contains("\"type\":\"query-request\""));
        assert!(json.contains("\"queryId\":\"query-456\""));
        assert!(json.contains("\"sql\":\"SELECT * FROM users\""));

        let deserialized: ChannelMessage = serde_json::from_str(&json).expect("Should deserialize");
        match deserialized {
            ChannelMessage::QueryRequest { query_id, sql } => {
                assert_eq!(query_id, "query-456");
                assert_eq!(sql, "SELECT * FROM users");
            }
            _ => panic!("Wrong message type deserialized"),
        }
    }

    #[wasm_bindgen_test]
    fn test_channel_message_query_response_success_serialization() {
        let msg = ChannelMessage::QueryResponse {
            query_id: "query-789".to_string(),
            result: Some("[{\"id\": 1, \"name\": \"test\"}]".to_string()),
            error: None,
        };

        let json = serde_json::to_string(&msg).expect("Should serialize");
        assert!(json.contains("\"type\":\"query-response\""));
        assert!(json.contains("\"queryId\":\"query-789\""));
        assert!(json.contains("\"result\":\"[{"));
        assert!(json.contains("\"error\":null"));

        let deserialized: ChannelMessage = serde_json::from_str(&json).expect("Should deserialize");
        match deserialized {
            ChannelMessage::QueryResponse {
                query_id,
                result,
                error,
            } => {
                assert_eq!(query_id, "query-789");
                assert!(result.is_some());
                assert!(error.is_none());
            }
            _ => panic!("Wrong message type deserialized"),
        }
    }

    #[wasm_bindgen_test]
    fn test_channel_message_query_response_error_serialization() {
        let msg = ChannelMessage::QueryResponse {
            query_id: "query-error".to_string(),
            result: None,
            error: Some("SQL syntax error".to_string()),
        };

        let json = serde_json::to_string(&msg).expect("Should serialize");
        assert!(json.contains("\"type\":\"query-response\""));
        assert!(json.contains("\"error\":\"SQL syntax error\""));
        assert!(json.contains("\"result\":null"));

        let deserialized: ChannelMessage = serde_json::from_str(&json).expect("Should deserialize");
        match deserialized {
            ChannelMessage::QueryResponse {
                query_id,
                result,
                error,
            } => {
                assert_eq!(query_id, "query-error");
                assert!(result.is_none());
                assert_eq!(error.unwrap(), "SQL syntax error");
            }
            _ => panic!("Wrong message type deserialized"),
        }
    }

    #[wasm_bindgen_test]
    fn test_worker_message_execute_query_serialization() {
        let msg = WorkerMessage::ExecuteQuery {
            sql: "INSERT INTO table VALUES (1, 'test')".to_string(),
        };

        let json = serde_json::to_string(&msg).expect("Should serialize");
        assert!(json.contains("\"type\":\"execute-query\""));
        assert!(json.contains("\"sql\":\"INSERT INTO table VALUES (1, 'test')\""));

        let deserialized: WorkerMessage = serde_json::from_str(&json).expect("Should deserialize");
        match deserialized {
            WorkerMessage::ExecuteQuery { sql } => {
                assert_eq!(sql, "INSERT INTO table VALUES (1, 'test')");
            }
        }
    }

    #[wasm_bindgen_test]
    fn test_main_thread_message_query_result_serialization() {
        let msg = MainThreadMessage::QueryResult {
            result: Some("Success".to_string()),
            error: None,
        };

        let json = serde_json::to_string(&msg).expect("Should serialize");
        assert!(json.contains("\"type\":\"query-result\""));
        assert!(json.contains("\"result\":\"Success\""));

        let deserialized: MainThreadMessage =
            serde_json::from_str(&json).expect("Should deserialize");
        match deserialized {
            MainThreadMessage::QueryResult { result, error } => {
                assert_eq!(result.unwrap(), "Success");
                assert!(error.is_none());
            }
            _ => panic!("Wrong message type deserialized"),
        }
    }

    #[wasm_bindgen_test]
    fn test_main_thread_message_worker_ready_serialization() {
        let msg = MainThreadMessage::WorkerReady;

        let json = serde_json::to_string(&msg).expect("Should serialize");
        assert!(json.contains("\"type\":\"worker-ready\""));

        let deserialized: MainThreadMessage =
            serde_json::from_str(&json).expect("Should deserialize");
        match deserialized {
            MainThreadMessage::WorkerReady => {
                // Success
            }
            _ => panic!("Wrong message type deserialized"),
        }
    }

    #[wasm_bindgen_test]
    fn test_invalid_message_deserialization() {
        let invalid_json = r#"{"type": "unknown-type", "data": "test"}"#;

        // Should fail gracefully for unknown message types
        let result = serde_json::from_str::<ChannelMessage>(invalid_json);
        assert!(result.is_err());
    }
}
