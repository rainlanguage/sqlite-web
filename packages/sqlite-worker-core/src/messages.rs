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
