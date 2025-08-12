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
