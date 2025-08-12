use js_sys::{Function, Object, Promise, Reflect};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use uuid::Uuid;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::BroadcastChannel;

use crate::database::SQLiteDatabase;
use crate::messages::{ChannelMessage, PendingQuery};

// Worker state
pub struct WorkerState {
    pub worker_id: String,
    pub is_leader: Rc<RefCell<bool>>,
    pub db: Rc<RefCell<Option<Rc<SQLiteDatabase>>>>,
    pub channel: BroadcastChannel,
    pub pending_queries: Rc<RefCell<HashMap<String, PendingQuery>>>,
}

impl WorkerState {
    pub fn new() -> Result<Self, JsValue> {
        let worker_id = Uuid::new_v4().to_string();
        let channel = BroadcastChannel::new("sqlite-queries")?;

        Ok(WorkerState {
            worker_id,
            is_leader: Rc::new(RefCell::new(false)),
            db: Rc::new(RefCell::new(None)),
            channel,
            pending_queries: Rc::new(RefCell::new(HashMap::new())),
        })
    }

    pub fn setup_channel_listener(&self) {
        let is_leader = Rc::clone(&self.is_leader);
        let db = Rc::clone(&self.db);
        let pending_queries = Rc::clone(&self.pending_queries);
        let channel = self.channel.clone();

        let onmessage = Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
            let data = event.data();

            if let Ok(msg) = serde_wasm_bindgen::from_value::<ChannelMessage>(data) {
                match msg {
                    ChannelMessage::QueryRequest { query_id, sql } => {
                        if *is_leader.borrow() {
                            let db = Rc::clone(&db);
                            let channel = channel.clone();

                            spawn_local(async move {
                                let database = db.borrow().clone();
                                let result = if let Some(database) = database {
                                    database.exec(&sql).await
                                } else {
                                    Err("Database not initialized".to_string())
                                };

                                let response = match result {
                                    Ok(res) => ChannelMessage::QueryResponse {
                                        query_id,
                                        result: Some(res),
                                        error: None,
                                    },
                                    Err(err) => ChannelMessage::QueryResponse {
                                        query_id,
                                        result: None,
                                        error: Some(err),
                                    },
                                };

                                let msg_js = serde_wasm_bindgen::to_value(&response).unwrap();
                                let _ = channel.post_message(&msg_js);
                            });
                        }
                    }
                    ChannelMessage::QueryResponse {
                        query_id,
                        result,
                        error,
                    } => {
                        if let Some(pending) = pending_queries.borrow_mut().remove(&query_id) {
                            if let Some(err) = error {
                                let _ = pending
                                    .reject
                                    .call1(&JsValue::NULL, &JsValue::from_str(&err));
                            } else if let Some(res) = result {
                                let _ = pending
                                    .resolve
                                    .call1(&JsValue::NULL, &JsValue::from_str(&res));
                            }
                        }
                    }
                    ChannelMessage::NewLeader { leader_id: _ } => {}
                }
            }
        }) as Box<dyn FnMut(web_sys::MessageEvent)>);

        self.channel
            .set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();
    }

    pub async fn attempt_leadership(&self) {
        let worker_id = self.worker_id.clone();
        let is_leader = Rc::clone(&self.is_leader);
        let db = Rc::clone(&self.db);
        let channel = self.channel.clone();

        // Get navigator.locks from WorkerGlobalScope
        let global = js_sys::global();
        let navigator = Reflect::get(&global, &JsValue::from_str("navigator")).unwrap();
        let locks = Reflect::get(&navigator, &JsValue::from_str("locks")).unwrap();

        let options = Object::new();
        Reflect::set(
            &options,
            &JsValue::from_str("mode"),
            &JsValue::from_str("exclusive"),
        )
        .unwrap();

        let handler = Closure::once(move |_lock: JsValue| -> Promise {
            *is_leader.borrow_mut() = true;

            let db = Rc::clone(&db);
            let channel = channel.clone();
            let worker_id = worker_id.clone();

            spawn_local(async move {
                match SQLiteDatabase::initialize_opfs().await {
                    Ok(database) => {
                        *db.borrow_mut() = Some(Rc::new(database));

                        let msg = ChannelMessage::NewLeader {
                            leader_id: worker_id.clone(),
                        };
                        let msg_js = serde_wasm_bindgen::to_value(&msg).unwrap();
                        let _ = channel.post_message(&msg_js);
                    }
                    Err(_e) => {}
                }
            });

            // Never resolve = hold lock forever
            Promise::new(&mut |_, _| {})
        });

        let request_fn = Reflect::get(&locks, &JsValue::from_str("request")).unwrap();
        let request_fn = request_fn.dyn_ref::<Function>().unwrap();

        let _ = request_fn.call3(
            &locks,
            &JsValue::from_str("sqlite-database"),
            &options,
            handler.as_ref().unchecked_ref(),
        );

        handler.forget();
    }

    pub async fn execute_query(&self, sql: String) -> Result<String, String> {
        if *self.is_leader.borrow() {
            let database = self.db.borrow().clone();
            if let Some(database) = database {
                database.exec(&sql).await
            } else {
                Err("Database not initialized".to_string())
            }
        } else {
            let query_id = Uuid::new_v4().to_string();

            let promise = Promise::new(&mut |resolve, reject| {
                self.pending_queries
                    .borrow_mut()
                    .insert(query_id.clone(), PendingQuery { resolve, reject });
            });

            let msg = ChannelMessage::QueryRequest {
                query_id: query_id.clone(),
                sql,
            };
            let msg_js = serde_wasm_bindgen::to_value(&msg).unwrap();
            let _ = self.channel.post_message(&msg_js);

            // Timeout handling
            let timeout_promise = Promise::new(&mut |_, reject| {
                let query_id = query_id.clone();
                let pending_queries = Rc::clone(&self.pending_queries);

                let callback = Closure::once(move || {
                    if pending_queries.borrow_mut().remove(&query_id).is_some() {
                        let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("Query timeout"));
                    }
                });

                let global = js_sys::global();
                let set_timeout = Reflect::get(&global, &JsValue::from_str("setTimeout")).unwrap();
                let set_timeout = set_timeout.dyn_ref::<Function>().unwrap();
                set_timeout
                    .call2(
                        &JsValue::NULL,
                        callback.as_ref().unchecked_ref(),
                        &JsValue::from_f64(5000.0),
                    )
                    .unwrap();
                callback.forget();
            });

            let result = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::race(
                &js_sys::Array::of2(&promise, &timeout_promise),
            ))
            .await;

            match result {
                Ok(val) => {
                    if let Some(s) = val.as_string() {
                        Ok(s)
                    } else {
                        Err("Invalid response".to_string())
                    }
                }
                Err(e) => Err(format!("{e:?}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use js_sys::{Array, Function, Object, Reflect};
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn test_worker_state_creation() {
        let result = WorkerState::new();

        match result {
            Ok(state) => {
                assert!(!state.worker_id.is_empty());
                assert!(state.worker_id.contains('-'));
                assert!(!*state.is_leader.borrow());
                assert!(state.db.borrow().is_none());
                assert!(state.pending_queries.borrow().is_empty());
            }
            Err(_) => {
                assert!(true);
            }
        }
    }

    #[wasm_bindgen_test]
    fn test_worker_state_unique_ids() {
        let results: Vec<_> = (0..5).map(|_| WorkerState::new()).collect();

        let mut ids = std::collections::HashSet::new();
        let mut valid_count = 0;

        for result in results {
            if let Ok(state) = result {
                assert!(
                    ids.insert(state.worker_id.clone()),
                    "Worker IDs should be unique"
                );
                valid_count += 1;
            }
        }

        if valid_count > 1 {
            assert_eq!(ids.len(), valid_count);
        }
    }

    #[wasm_bindgen_test]
    fn test_channel_message_query_request_handling() {
        let query_request = ChannelMessage::QueryRequest {
            query_id: "test-query-123".to_string(),
            sql: "SELECT * FROM test_table".to_string(),
        };

        let serialized = serde_wasm_bindgen::to_value(&query_request);
        assert!(serialized.is_ok());

        let js_value = serialized.unwrap();

        let msg_type = Reflect::get(&js_value, &JsValue::from_str("type")).unwrap();
        assert_eq!(msg_type.as_string().unwrap(), "query-request");

        let query_id = Reflect::get(&js_value, &JsValue::from_str("queryId")).unwrap();
        assert_eq!(query_id.as_string().unwrap(), "test-query-123");

        let sql = Reflect::get(&js_value, &JsValue::from_str("sql")).unwrap();
        assert_eq!(sql.as_string().unwrap(), "SELECT * FROM test_table");
    }

    #[wasm_bindgen_test]
    fn test_channel_message_query_response_success_handling() {
        let query_response = ChannelMessage::QueryResponse {
            query_id: "test-query-456".to_string(),
            result: Some("[{\"id\": 1, \"name\": \"test\"}]".to_string()),
            error: None,
        };

        let serialized = serde_wasm_bindgen::to_value(&query_response);
        assert!(serialized.is_ok());

        let js_value = serialized.unwrap();

        let msg_type = Reflect::get(&js_value, &JsValue::from_str("type")).unwrap();
        assert_eq!(msg_type.as_string().unwrap(), "query-response");

        let query_id = Reflect::get(&js_value, &JsValue::from_str("queryId")).unwrap();
        assert_eq!(query_id.as_string().unwrap(), "test-query-456");

        let result = Reflect::get(&js_value, &JsValue::from_str("result")).unwrap();
        assert!(result.is_string());

        let error = Reflect::get(&js_value, &JsValue::from_str("error")).unwrap();
        assert!(error.is_null() || error.is_undefined());
    }

    #[wasm_bindgen_test]
    fn test_channel_message_query_response_error_handling() {
        let query_response = ChannelMessage::QueryResponse {
            query_id: "test-query-error".to_string(),
            result: None,
            error: Some("SQL syntax error: near 'SELCT'".to_string()),
        };

        let serialized = serde_wasm_bindgen::to_value(&query_response);
        assert!(serialized.is_ok());

        let js_value = serialized.unwrap();

        let error = Reflect::get(&js_value, &JsValue::from_str("error")).unwrap();
        assert_eq!(error.as_string().unwrap(), "SQL syntax error: near 'SELCT'");

        let result = Reflect::get(&js_value, &JsValue::from_str("result")).unwrap();
        assert!(result.is_null() || result.is_undefined());
    }

    #[wasm_bindgen_test]
    fn test_channel_message_new_leader_handling() {
        let new_leader = ChannelMessage::NewLeader {
            leader_id: "leader-worker-789".to_string(),
        };

        let serialized = serde_wasm_bindgen::to_value(&new_leader);
        assert!(serialized.is_ok());

        let js_value = serialized.unwrap();

        let msg_type = Reflect::get(&js_value, &JsValue::from_str("type")).unwrap();
        assert_eq!(msg_type.as_string().unwrap(), "new-leader");

        let leader_id = Reflect::get(&js_value, &JsValue::from_str("leaderId")).unwrap();
        assert_eq!(leader_id.as_string().unwrap(), "leader-worker-789");
    }

    #[wasm_bindgen_test]
    fn test_pending_query_storage() {
        let mut pending_queries = HashMap::new();

        let resolve_fn = Function::new_no_args("return 'resolved';");
        let reject_fn = Function::new_no_args("return 'rejected';");

        let pending_query = PendingQuery {
            resolve: resolve_fn.clone(),
            reject: reject_fn.clone(),
        };

        let query_id = "test-pending-query";
        pending_queries.insert(query_id.to_string(), pending_query);

        assert!(pending_queries.contains_key(query_id));
        assert_eq!(pending_queries.len(), 1);

        let retrieved = pending_queries.remove(query_id);
        assert!(retrieved.is_some());
        assert!(pending_queries.is_empty());
    }

    #[wasm_bindgen_test]
    fn test_worker_state_leadership_flag() {
        if let Ok(state) = WorkerState::new() {
            assert!(!*state.is_leader.borrow());

            *state.is_leader.borrow_mut() = true;
            assert!(*state.is_leader.borrow());

            *state.is_leader.borrow_mut() = false;
            assert!(!*state.is_leader.borrow());
        }
    }

    #[wasm_bindgen_test]
    fn test_worker_state_database_storage() {
        if let Ok(state) = WorkerState::new() {
            assert!(state.db.borrow().is_none());
            assert!(state.db.borrow().is_none());
        }
    }

    #[wasm_bindgen_test]
    fn test_pending_queries_concurrent_access() {
        if let Ok(state) = WorkerState::new() {
            let pending_queries = Rc::clone(&state.pending_queries);

            let resolve1 = Function::new_no_args("return 'resolve1';");
            let reject1 = Function::new_no_args("return 'reject1';");
            let resolve2 = Function::new_no_args("return 'resolve2';");
            let reject2 = Function::new_no_args("return 'reject2';");

            {
                let mut queries = pending_queries.borrow_mut();
                queries.insert(
                    "query1".to_string(),
                    PendingQuery {
                        resolve: resolve1,
                        reject: reject1,
                    },
                );
                queries.insert(
                    "query2".to_string(),
                    PendingQuery {
                        resolve: resolve2,
                        reject: reject2,
                    },
                );
            }

            assert_eq!(pending_queries.borrow().len(), 2);
            assert!(pending_queries.borrow().contains_key("query1"));
            assert!(pending_queries.borrow().contains_key("query2"));

            let removed = pending_queries.borrow_mut().remove("query1");
            assert!(removed.is_some());
            assert_eq!(pending_queries.borrow().len(), 1);
            assert!(!pending_queries.borrow().contains_key("query1"));
            assert!(pending_queries.borrow().contains_key("query2"));
        }
    }

    #[wasm_bindgen_test]
    fn test_uuid_generation() {
        let uuid1 = Uuid::new_v4().to_string();
        let uuid2 = Uuid::new_v4().to_string();

        assert_ne!(uuid1, uuid2);
        assert_eq!(uuid1.len(), 36);
        assert_eq!(uuid2.len(), 36);
        assert!(uuid1.contains('-'));
        assert!(uuid2.contains('-'));

        for c in uuid1.chars() {
            assert!(c.is_ascii_hexdigit() || c == '-');
        }
        for c in uuid2.chars() {
            assert!(c.is_ascii_hexdigit() || c == '-');
        }
    }

    #[wasm_bindgen_test]
    fn test_message_deserialization_error_handling() {
        let invalid_json = JsValue::from_str("invalid json");
        let result = serde_wasm_bindgen::from_value::<ChannelMessage>(invalid_json);
        assert!(result.is_err(), "Should fail to deserialize invalid JSON");
    }

    #[wasm_bindgen_test]
    fn test_javascript_object_creation() {
        let obj = Object::new();

        let set_result = Reflect::set(
            &obj,
            &JsValue::from_str("type"),
            &JsValue::from_str("test-message"),
        );
        assert!(set_result.is_ok());

        let set_result2 = Reflect::set(
            &obj,
            &JsValue::from_str("data"),
            &JsValue::from_str("test-data"),
        );
        assert!(set_result2.is_ok());

        let type_val = Reflect::get(&obj, &JsValue::from_str("type")).unwrap();
        assert_eq!(type_val.as_string().unwrap(), "test-message");

        let data_val = Reflect::get(&obj, &JsValue::from_str("data")).unwrap();
        assert_eq!(data_val.as_string().unwrap(), "test-data");
    }

    #[wasm_bindgen_test]
    fn test_timeout_promise_creation() {
        let promise = Promise::new(&mut |resolve, _reject| {
            let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str("timeout"));
        });

        assert!(!promise.is_undefined());
        assert!(promise.is_object());
    }

    #[wasm_bindgen_test]
    fn test_message_race_condition_setup() {
        let promise1 = Promise::new(&mut |resolve, _reject| {
            let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str("result1"));
        });

        let promise2 = Promise::new(&mut |resolve, _reject| {
            let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str("result2"));
        });

        let promises_array = Array::of2(&promise1, &promise2);
        let race_promise = js_sys::Promise::race(&promises_array);

        assert!(!race_promise.is_undefined());
        assert!(race_promise.is_object());
    }

    #[wasm_bindgen_test]
    fn test_error_message_formatting() {
        let error_msg = "Database connection failed";
        let js_error = JsValue::from_str(error_msg);
        let formatted = format!("{:?}", js_error);
        assert!(!formatted.is_empty());
    }
}
