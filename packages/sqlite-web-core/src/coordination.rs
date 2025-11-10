use js_sys::{Function, Object, Promise, Reflect};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use uuid::Uuid;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{BroadcastChannel, DedicatedWorkerGlobalScope};

use crate::database::SQLiteDatabase;
use crate::messages::{ChannelMessage, PendingQuery};
use crate::util::{js_value_to_string, sanitize_identifier, set_js_property};

// Worker state
pub struct WorkerState {
    pub worker_id: String,
    pub is_leader: Rc<RefCell<bool>>,
    pub has_leader: Rc<RefCell<bool>>,
    pub db: Rc<RefCell<Option<SQLiteDatabase>>>,
    pub channel: BroadcastChannel,
    pub db_name: String,
    pub pending_queries: Rc<RefCell<HashMap<String, PendingQuery>>>,
    pub follower_timeout_ms: f64,
}

fn reflect_get(target: &JsValue, key: &str) -> Result<JsValue, JsValue> {
    Reflect::get(target, &JsValue::from_str(key))
}

fn send_channel_message(
    channel: &BroadcastChannel,
    message: &ChannelMessage,
) -> Result<(), String> {
    let value = serde_wasm_bindgen::to_value(message)
        .map_err(|err| format!("Failed to serialize channel message: {err:?}"))?;
    channel.post_message(&value).map_err(|err| {
        format!(
            "Failed to post channel message: {}",
            js_value_to_string(&err)
        )
    })
}

fn post_worker_message(obj: &js_sys::Object) -> Result<(), String> {
    let global = js_sys::global();
    let scope: DedicatedWorkerGlobalScope = global
        .dyn_into()
        .map_err(|_| "Failed to access worker scope".to_string())?;
    scope
        .post_message(obj.as_ref())
        .map_err(|err| js_value_to_string(&err))
}

fn send_worker_ready_message() -> Result<(), String> {
    let message = js_sys::Object::new();
    set_js_property(&message, "type", &JsValue::from_str("worker-ready"))
        .map_err(|err| js_value_to_string(&err))?;
    post_worker_message(&message)
}

fn send_worker_error_message(error: &str) -> Result<(), String> {
    let message = js_sys::Object::new();
    set_js_property(&message, "type", &JsValue::from_str("worker-error"))
        .map_err(|err| js_value_to_string(&err))?;
    set_js_property(&message, "error", &JsValue::from_str(error))
        .map_err(|err| js_value_to_string(&err))?;
    post_worker_message(&message)
}

impl WorkerState {
    pub fn new() -> Result<Self, JsValue> {
        fn get_db_name_from_global() -> Result<String, JsValue> {
            let global = js_sys::global();
            let val = Reflect::get(&global, &JsValue::from_str("__SQLITE_DB_NAME"))
                .unwrap_or(JsValue::UNDEFINED);
            if let Some(s) = val.as_string() {
                let trimmed = s.trim().to_string();
                if trimmed.is_empty() {
                    return Err(JsValue::from_str("Database name is required"));
                }
                Ok(trimmed)
            } else {
                #[cfg(test)]
                {
                    return Ok("testdb".to_string());
                }
                #[allow(unreachable_code)]
                Err(JsValue::from_str("Database name is required"))
            }
        }

        fn get_follower_timeout_from_global() -> f64 {
            let global = js_sys::global();
            let val = Reflect::get(&global, &JsValue::from_str("__SQLITE_FOLLOWER_TIMEOUT_MS"))
                .unwrap_or(JsValue::UNDEFINED);
            if let Some(n) = val.as_f64() {
                if n.is_finite() && n >= 0.0 {
                    return n;
                }
            }
            5000.0
        }

        let worker_id = Uuid::new_v4().to_string();
        let db_name_raw = get_db_name_from_global()?;
        let channel_name = format!("sqlite-queries-{}", sanitize_identifier(&db_name_raw));
        let channel = BroadcastChannel::new(&channel_name)?;
        let follower_timeout_ms = get_follower_timeout_from_global();

        Ok(WorkerState {
            worker_id,
            is_leader: Rc::new(RefCell::new(false)),
            has_leader: Rc::new(RefCell::new(false)),
            db: Rc::new(RefCell::new(None)),
            channel,
            db_name: db_name_raw,
            pending_queries: Rc::new(RefCell::new(HashMap::new())),
            follower_timeout_ms,
        })
    }

    pub fn setup_channel_listener(&self) -> Result<(), JsValue> {
        let is_leader = Rc::clone(&self.is_leader);
        let has_leader = Rc::clone(&self.has_leader);
        let db = Rc::clone(&self.db);
        let pending_queries = Rc::clone(&self.pending_queries);
        let channel = self.channel.clone();
        let worker_id = self.worker_id.clone();

        let onmessage = Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
            let data = event.data();
            if let Ok(msg) = serde_wasm_bindgen::from_value::<ChannelMessage>(data) {
                handle_channel_message(
                    &is_leader,
                    &has_leader,
                    &db,
                    &channel,
                    &pending_queries,
                    &worker_id,
                    msg,
                );
            }
        }) as Box<dyn FnMut(web_sys::MessageEvent)>);

        self.channel
            .set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();
        Ok(())
    }

    pub fn start_leader_probe(self: &Rc<Self>) {
        if *self.is_leader.borrow() {
            return;
        }
        let has_leader = Rc::clone(&self.has_leader);
        let channel = self.channel.clone();
        let worker_id = self.worker_id.clone();
        spawn_local(async move {
            const MAX_ATTEMPTS: u32 = 40;
            let mut attempts = 0;
            while attempts < MAX_ATTEMPTS {
                attempts += 1;
                if *has_leader.borrow() {
                    break;
                }
                let ping = ChannelMessage::LeaderPing {
                    requester_id: worker_id.clone(),
                };
                if let Err(err_msg) = send_channel_message(&channel, &ping) {
                    let _ = send_worker_error_message(&err_msg);
                    break;
                }
                sleep_ms(250).await;
            }
        });
    }

    pub async fn attempt_leadership(&self) -> Result<(), JsValue> {
        let worker_id = self.worker_id.clone();
        let is_leader = Rc::clone(&self.is_leader);
        let has_leader = Rc::clone(&self.has_leader);
        let db = Rc::clone(&self.db);
        let channel = self.channel.clone();
        let db_name_for_handler = self.db_name.clone();

        // Get navigator.locks from WorkerGlobalScope
        let global = js_sys::global();
        let navigator = reflect_get(&global, "navigator")?;
        let locks = reflect_get(&navigator, "locks")?;

        let options = Object::new();
        set_js_property(&options, "mode", &JsValue::from_str("exclusive"))?;

        let handler = Closure::once(move |_lock: JsValue| -> Promise {
            *is_leader.borrow_mut() = true;
            *has_leader.borrow_mut() = true;

            let db = Rc::clone(&db);
            let channel = channel.clone();
            let worker_id = worker_id.clone();
            let db_name = db_name_for_handler.clone();
            let has_leader_inner = Rc::clone(&has_leader);

            spawn_local(async move {
                match SQLiteDatabase::initialize_opfs(&db_name).await {
                    Ok(database) => {
                        *db.borrow_mut() = Some(database);
                        *has_leader_inner.borrow_mut() = true;

                        let msg = ChannelMessage::NewLeader {
                            leader_id: worker_id.clone(),
                        };
                        if let Err(err_msg) = send_channel_message(&channel, &msg) {
                            let fallback = ChannelMessage::QueryResponse {
                                query_id: worker_id.clone(),
                                result: None,
                                error: Some(err_msg),
                            };
                            let _ = send_channel_message(&channel, &fallback);
                        }
                        if let Err(err_msg) = send_worker_ready_message() {
                            let _ = send_worker_error_message(&err_msg);
                        }
                    }
                    Err(err) => {
                        let msg = js_value_to_string(&err);
                        *has_leader_inner.borrow_mut() = false;
                        let _ = send_worker_error_message(&msg);
                    }
                }
            });

            // Never resolve = hold lock forever
            Promise::new(&mut |_, _| {})
        });

        let request_fn = reflect_get(&locks, "request")?;
        let request_fn = request_fn
            .dyn_ref::<Function>()
            .ok_or_else(|| JsValue::from_str("navigator.locks.request is not a function"))?;

        let lock_id: String = format!("sqlite-database-{}", sanitize_identifier(&self.db_name));
        request_fn.call3(
            &locks,
            &JsValue::from_str(&lock_id),
            &options,
            handler.as_ref().unchecked_ref(),
        )?;

        handler.forget();
        Ok(())
    }

    pub async fn execute_query(
        &self,
        sql: String,
        params: Option<Vec<serde_json::Value>>,
    ) -> Result<String, String> {
        if *self.is_leader.borrow() {
            exec_on_db(Rc::clone(&self.db), sql, params).await
        } else {
            if !*self.has_leader.borrow() {
                return Err("InitializationPending".to_string());
            }
            let query_id = Uuid::new_v4().to_string();

            let promise = Promise::new(&mut |resolve, reject| {
                self.pending_queries
                    .borrow_mut()
                    .insert(query_id.clone(), PendingQuery { resolve, reject });
            });

            post_query_request(&self.channel, &query_id, sql, params)?;

            let timeout_promise = schedule_timeout_promise(
                Rc::clone(&self.pending_queries),
                query_id.clone(),
                self.follower_timeout_ms,
            );

            let result = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::race(
                &js_sys::Array::of2(&promise, &timeout_promise),
            ))
            .await;

            match result {
                Ok(val) => val
                    .as_string()
                    .ok_or_else(|| "Invalid response".to_string()),
                Err(e) => Err(js_value_to_string(&e)),
            }
        }
    }
}

fn handle_channel_message(
    is_leader: &Rc<RefCell<bool>>,
    has_leader: &Rc<RefCell<bool>>,
    db: &Rc<RefCell<Option<SQLiteDatabase>>>,
    channel: &BroadcastChannel,
    pending_queries: &Rc<RefCell<HashMap<String, PendingQuery>>>,
    worker_id: &str,
    msg: ChannelMessage,
) {
    match msg {
        ChannelMessage::QueryRequest {
            query_id,
            sql,
            params,
        } => {
            if *is_leader.borrow() {
                let db = Rc::clone(db);
                let channel = channel.clone();
                spawn_local(async move {
                    let result = exec_on_db(db, sql, params).await;
                    let response = build_query_response(query_id.clone(), result);
                    if let Err(err_msg) = send_channel_message(&channel, &response) {
                        let fallback = ChannelMessage::QueryResponse {
                            query_id: query_id.clone(),
                            result: None,
                            error: Some(err_msg),
                        };
                        let _ = send_channel_message(&channel, &fallback);
                    }
                });
            }
        }
        ChannelMessage::QueryResponse {
            query_id,
            result,
            error,
        } => handle_query_response(pending_queries, query_id, result, error),
        ChannelMessage::NewLeader { leader_id: _ } => {
            let mut has_leader_ref = has_leader.borrow_mut();
            let already_had_leader = *has_leader_ref;
            *has_leader_ref = true;
            drop(has_leader_ref);

            if !already_had_leader {
                if let Err(err_msg) = send_worker_ready_message() {
                    let _ = send_worker_error_message(&err_msg);
                }
            }
        }
        ChannelMessage::LeaderPing { requester_id: _ } => {
            if *is_leader.borrow() {
                let response = ChannelMessage::NewLeader {
                    leader_id: worker_id.to_string(),
                };
                if let Err(err_msg) = send_channel_message(channel, &response) {
                    let _ = send_worker_error_message(&err_msg);
                }
            }
        }
    }
}

fn build_query_response(query_id: String, result: Result<String, String>) -> ChannelMessage {
    match result {
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
    }
}

fn handle_query_response(
    pending_queries: &Rc<RefCell<HashMap<String, PendingQuery>>>,
    query_id: String,
    result: Option<String>,
    error: Option<String>,
) {
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

async fn sleep_ms(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let resolve_for_timeout = resolve.clone();
        let closure = Closure::once(move || {
            let _ = resolve_for_timeout.call0(&JsValue::NULL);
        });

        let timeout_result = js_sys::global()
            .dyn_into::<DedicatedWorkerGlobalScope>()
            .map_err(|_| ())
            .and_then(|scope| {
                scope
                    .set_timeout_with_callback_and_timeout_and_arguments_0(
                        closure.as_ref().unchecked_ref(),
                        ms,
                    )
                    .map(|_| ())
                    .map_err(|_| ())
            })
            .or_else(|_| {
                web_sys::window().ok_or(()).and_then(|win| {
                    win.set_timeout_with_callback_and_timeout_and_arguments_0(
                        closure.as_ref().unchecked_ref(),
                        ms,
                    )
                    .map(|_| ())
                    .map_err(|_| ())
                })
            });

        if timeout_result.is_err() {
            // As a best-effort fallback, resolve immediately.
            let _ = resolve.call0(&JsValue::NULL);
        }

        closure.forget();
    });
    let _ = JsFuture::from(promise).await;
}

async fn exec_on_db(
    db: Rc<RefCell<Option<SQLiteDatabase>>>,
    sql: String,
    params: Option<Vec<serde_json::Value>>,
) -> Result<String, String> {
    let db_opt = db.borrow_mut().take();
    let result = match db_opt {
        Some(mut database) => {
            let result = match params {
                Some(p) => database.exec_with_params(&sql, p).await,
                None => database.exec(&sql).await,
            };
            *db.borrow_mut() = Some(database);
            result
        }
        None => Err("Database not initialized".to_string()),
    };
    result
}

fn post_query_request(
    channel: &BroadcastChannel,
    query_id: &str,
    sql: String,
    params: Option<Vec<serde_json::Value>>,
) -> Result<(), String> {
    let msg = ChannelMessage::QueryRequest {
        query_id: query_id.to_string(),
        sql,
        params,
    };
    let msg_js = serde_wasm_bindgen::to_value(&msg)
        .map_err(|e| format!("Failed to serialize query request: {e:?}"))?;
    channel
        .post_message(&msg_js)
        .map_err(|e| format!("Failed to post query request: {e:?}"))
}

fn schedule_timeout_promise(
    pending_queries: Rc<RefCell<HashMap<String, PendingQuery>>>,
    query_id: String,
    ms: f64,
) -> Promise {
    Promise::new(&mut move |_, reject| {
        let query_id = query_id.clone();
        let pending_queries = Rc::clone(&pending_queries);
        let callback = Closure::once(move || {
            if pending_queries.borrow_mut().remove(&query_id).is_some() {
                let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("Query timeout"));
            }
        });

        let global = js_sys::global();
        if let Ok(set_timeout_value) = reflect_get(&global, "setTimeout") {
            if let Some(set_timeout_fn) = set_timeout_value.dyn_ref::<Function>() {
                let _ = set_timeout_fn.call2(
                    &JsValue::NULL,
                    callback.as_ref().unchecked_ref(),
                    &JsValue::from_f64(ms),
                );
            }
        }
        callback.forget();
    })
}

#[cfg(all(test, target_family = "wasm"))]
mod tests {
    use super::*;
    use js_sys::Function;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn test_worker_state_creation_and_uniqueness() {
        let results: Vec<_> = (0..5).map(|_| WorkerState::new()).collect();
        let workers: Vec<_> = results.into_iter().filter_map(Result::ok).collect();

        assert!(!workers.is_empty(), "Should create at least one worker");

        let state = &workers[0];
        assert!(!state.worker_id.is_empty(), "Worker ID should not be empty");
        assert!(
            state.worker_id.contains('-'),
            "Worker ID should be valid UUID format"
        );
        assert!(
            !*state.is_leader.borrow(),
            "New workers should not start as leader"
        );
        assert!(
            state.db.borrow().is_none(),
            "Database should be uninitialized"
        );
        assert!(
            state.pending_queries.borrow().is_empty(),
            "Should have no pending queries"
        );

        if workers.len() >= 2 {
            let mut ids = std::collections::HashSet::new();
            for worker in &workers {
                assert!(
                    ids.insert(worker.worker_id.clone()),
                    "Worker ID {} should be unique",
                    worker.worker_id
                );
                assert_eq!(worker.worker_id.len(), 36, "UUID should be 36 characters");
                assert_eq!(
                    worker.worker_id.chars().filter(|&c| c == '-').count(),
                    4,
                    "UUID should have 4 dashes"
                );
            }
            assert_eq!(ids.len(), workers.len(), "All worker IDs should be unique");
        }
    }

    #[wasm_bindgen_test]
    fn test_leadership_state_management() {
        if let Ok(state) = WorkerState::new() {
            assert!(!*state.is_leader.borrow(), "Should start as follower");

            *state.is_leader.borrow_mut() = true;
            assert!(*state.is_leader.borrow(), "Should become leader");

            *state.is_leader.borrow_mut() = false;
            assert!(!*state.is_leader.borrow(), "Should become follower again");
        }
    }

    #[wasm_bindgen_test]
    fn test_pending_queries_management() {
        if let Ok(state) = WorkerState::new() {
            let pending_queries = Rc::clone(&state.pending_queries);

            assert_eq!(pending_queries.borrow().len(), 0);

            let test_queries = vec!["query-a", "query-b", "query-c", "query-d", "query-e"];
            {
                let mut queries = pending_queries.borrow_mut();
                for query_id in &test_queries {
                    let resolve =
                        Function::new_no_args(&format!("return 'resolved-{}';", query_id));
                    let reject = Function::new_no_args(&format!("return 'rejected-{}';", query_id));
                    queries.insert(query_id.to_string(), PendingQuery { resolve, reject });
                }
            }

            assert_eq!(pending_queries.borrow().len(), test_queries.len());
            for query_id in &test_queries {
                assert!(pending_queries.borrow().contains_key(*query_id));
            }

            for (i, query_id) in test_queries.iter().enumerate() {
                if i % 2 == 0 {
                    let removed = pending_queries.borrow_mut().remove(*query_id);
                    assert!(removed.is_some(), "Should remove query {}", query_id);
                }
            }

            let remaining_count = test_queries.len() - test_queries.len().div_ceil(2);
            assert_eq!(pending_queries.borrow().len(), remaining_count);

            pending_queries.borrow_mut().clear();
            assert_eq!(pending_queries.borrow().len(), 0);

            {
                let mut queries = pending_queries.borrow_mut();
                let resolve = Function::new_no_args("return 'post-cleanup';");
                let reject = Function::new_no_args("return 'rejected';");
                queries.insert(
                    "post-cleanup-test".to_string(),
                    PendingQuery { resolve, reject },
                );
            }
            assert_eq!(pending_queries.borrow().len(), 1);
            assert!(pending_queries.borrow().contains_key("post-cleanup-test"));
        }
    }

    #[wasm_bindgen_test]
    fn test_message_deserialization_error_handling() {
        let invalid_json = JsValue::from_str("invalid json");
        let result = serde_wasm_bindgen::from_value::<ChannelMessage>(invalid_json);
        assert!(result.is_err(), "Should fail to deserialize invalid JSON");
    }

    #[wasm_bindgen_test]
    async fn test_execute_query_leader_vs_follower_paths() {
        if let Ok(leader_state) = WorkerState::new() {
            *leader_state.is_leader.borrow_mut() = true;

            let test_queries = vec![
                "",
                "SELECT 1",
                "INSERT INTO test VALUES (1, 'hello')",
                "SELECT 'test with spaces and symbols: !@#$%^&*()'",
                "SELECT 'Hello 世界 🌍'",
            ];

            for query in test_queries {
                let result = leader_state.execute_query(query.to_string(), None).await;
                match result {
                    Err(msg) => assert_eq!(
                        msg, "Database not initialized",
                        "Leader should get DB init error for query: {}",
                        query
                    ),
                    Ok(_) => panic!(
                        "Expected database not initialized error for query: {}",
                        query
                    ),
                }
            }
        }

        if let Ok(follower_state) = WorkerState::new() {
            assert!(
                !*follower_state.is_leader.borrow(),
                "Should start as follower"
            );

            let result = follower_state
                .execute_query("SELECT 1".to_string(), None)
                .await;
            match result {
                Err(msg) => assert_eq!(
                    msg, "InitializationPending",
                    "Follower should reject while leader is pending"
                ),
                Ok(_) => panic!("Expected initialization error for follower"),
            }
        }
    }

    #[wasm_bindgen_test]
    fn test_setup_channel_listener() {
        if let Ok(state) = WorkerState::new() {
            assert!(
                state.setup_channel_listener().is_ok(),
                "setup_channel_listener should succeed"
            );
        }
    }

    #[wasm_bindgen_test]
    async fn test_attempt_leadership_behavior() {
        if let Ok(state) = WorkerState::new() {
            assert!(!*state.is_leader.borrow(), "Should start as follower");
            assert!(
                state.db.borrow().is_none(),
                "Database should be uninitialized"
            );

            let _ = state.attempt_leadership().await;
        }

        let workers: Vec<_> = (0..3).filter_map(|_| WorkerState::new().ok()).collect();
        if workers.len() >= 2 {
            for worker in &workers {
                assert!(!*worker.is_leader.borrow(), "All should start as followers");
            }

            for worker in &workers {
                let _ = worker.attempt_leadership().await;
            }
        }
    }

    #[wasm_bindgen_test]
    fn test_worker_state_rc_shared_references() {
        if let Ok(state) = WorkerState::new() {
            let is_leader_clone = Rc::clone(&state.is_leader);
            let pending_clone = Rc::clone(&state.pending_queries);

            assert_eq!(*state.is_leader.borrow(), *is_leader_clone.borrow());
            assert_eq!(
                state.pending_queries.borrow().len(),
                pending_clone.borrow().len()
            );

            *state.is_leader.borrow_mut() = true;
            assert!(
                *is_leader_clone.borrow(),
                "Changes should be visible through cloned Rc"
            );

            {
                let resolve = Function::new_no_args("return 'resolved';");
                let reject = Function::new_no_args("return 'rejected';");
                pending_clone
                    .borrow_mut()
                    .insert("test-ref".to_string(), PendingQuery { resolve, reject });
            }
            assert_eq!(
                state.pending_queries.borrow().len(),
                1,
                "Should see changes through original Rc"
            );
        }
    }

    #[wasm_bindgen_test(async)]
    async fn test_sleep_ms_completes() {
        sleep_ms(0).await;
    }
}
