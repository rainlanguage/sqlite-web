use js_sys::{Function, Object, Promise, Reflect};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use uuid::Uuid;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
#[cfg(all(test, target_family = "wasm"))]
use wasm_bindgen_test::*;
use web_sys::{
    Blob, BlobPropertyBag, BroadcastChannel, DedicatedWorkerGlobalScope, MessageEvent, Url, Worker,
};

use crate::database::SQLiteDatabase;
use crate::messages::{
    ChannelMessage, MainThreadMessage, WorkerErrorPayload, WorkerMessage,
    WORKER_ERROR_TYPE_INITIALIZATION_PENDING,
};
use crate::util::{js_value_to_string, sanitize_identifier, set_js_property};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeadershipRole {
    Leader,
    Follower,
}

pub struct WorkerConfig {
    pub db_name: String,
    pub follower_timeout_ms: f64,
}

pub fn worker_config_from_global() -> Result<WorkerConfig, JsValue> {
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

    Ok(WorkerConfig {
        db_name: get_db_name_from_global()?,
        follower_timeout_ms: get_follower_timeout_from_global(),
    })
}

enum DbRequestOrigin {
    Local { request_id: u32 },
    Forwarded { query_id: String },
}

struct DbJob {
    request_id: u32,
    sql: String,
    params: Option<Vec<serde_json::Value>>,
}

pub struct CoordinatorState {
    pub worker_id: String,
    pub role: Rc<RefCell<LeadershipRole>>,
    pub leader_id: Rc<RefCell<Option<String>>>,
    pub leader_ready: Rc<RefCell<bool>>,
    pub ready_signaled: Rc<RefCell<bool>>,
    pub follower_timeout_ms: f64,
    pub channel: BroadcastChannel,
    pub db_worker_ready: Rc<RefCell<bool>>,
    pub db_worker: Rc<RefCell<Option<Worker>>>,
    pub db_name: String,
    db_pending: Rc<RefCell<HashMap<u32, DbRequestOrigin>>>,
    pub follower_pending: Rc<RefCell<HashMap<String, u32>>>,
    pub next_db_request_id: Rc<RefCell<u32>>,
}

pub struct DbWorkerState {
    pub db: Rc<RefCell<Option<SQLiteDatabase>>>,
    pub db_name: String,
    db_queue: Rc<RefCell<VecDeque<DbJob>>>,
    db_processing: Rc<Cell<bool>>,
}

pub fn create_broadcast_channel(db_name: &str) -> Result<BroadcastChannel, JsValue> {
    let channel_name = format!("sqlite-queries-{}", sanitize_identifier(db_name));
    BroadcastChannel::new(&channel_name)
}

impl CoordinatorState {
    pub fn new(config: WorkerConfig) -> Result<Rc<Self>, JsValue> {
        Ok(Rc::new(CoordinatorState {
            worker_id: Uuid::new_v4().to_string(),
            role: Rc::new(RefCell::new(LeadershipRole::Follower)),
            leader_id: Rc::new(RefCell::new(None)),
            leader_ready: Rc::new(RefCell::new(false)),
            ready_signaled: Rc::new(RefCell::new(false)),
            follower_timeout_ms: config.follower_timeout_ms,
            channel: create_broadcast_channel(&config.db_name)?,
            db_worker_ready: Rc::new(RefCell::new(false)),
            db_worker: Rc::new(RefCell::new(None)),
            db_name: config.db_name,
            db_pending: Rc::new(RefCell::new(HashMap::new())),
            follower_pending: Rc::new(RefCell::new(HashMap::new())),
            next_db_request_id: Rc::new(RefCell::new(1)),
        }))
    }

    pub fn setup_channel_listener(self: &Rc<Self>) -> Result<(), JsValue> {
        let state = Rc::clone(self);
        let onmessage = Closure::wrap(Box::new(move |event: MessageEvent| {
            if let Ok(msg) = serde_wasm_bindgen::from_value::<ChannelMessage>(event.data()) {
                state.handle_channel_message(msg);
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        self.channel
            .set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();
        Ok(())
    }

    pub fn start_leader_probe(self: &Rc<Self>) {
        if matches!(*self.role.borrow(), LeadershipRole::Leader) {
            return;
        }
        let has_leader = Rc::clone(&self.leader_id);
        let timeout_ms = self.follower_timeout_ms;
        let worker_id = self.worker_id.clone();
        let channel = self.channel.clone();
        spawn_local(async move {
            const POLL_INTERVAL_MS: f64 = 250.0;
            let mut remaining_ms = if timeout_ms.is_finite() {
                timeout_ms.max(0.0)
            } else {
                f64::INFINITY
            };

            if remaining_ms <= 0.0 {
                if has_leader.borrow().is_none() {
                    let message = format!(
                        "Leader election timed out after {:.0}ms",
                        timeout_ms.max(0.0)
                    );
                    let _ = send_worker_error_message(&message);
                }
                return;
            }

            while remaining_ms.is_infinite() || remaining_ms > 0.0 {
                if has_leader.borrow().is_some() {
                    break;
                }
                let ping = ChannelMessage::LeaderPing {
                    requester_id: worker_id.clone(),
                };
                if let Err(err_msg) = send_channel_message(&channel, &ping) {
                    let _ = send_worker_error_message(&err_msg);
                    break;
                }

                let sleep_duration = if remaining_ms.is_infinite() {
                    POLL_INTERVAL_MS
                } else {
                    remaining_ms.min(POLL_INTERVAL_MS)
                };
                if sleep_duration <= 0.0 {
                    break;
                }
                sleep_ms(sleep_duration.ceil() as i32).await;
                if remaining_ms.is_finite() {
                    remaining_ms -= sleep_duration;
                }
            }
            if has_leader.borrow().is_none() {
                let timeout = timeout_ms.max(0.0);
                let message = format!("Leader election timed out after {:.0}ms", timeout);
                let _ = send_worker_error_message(&message);
            }
        });
    }

    pub fn try_become_leader(self: &Rc<Self>) {
        let state = Rc::clone(self);
        spawn_local(async move {
            if let Err(err) = state.acquire_lock_and_promote().await {
                let _ = send_worker_error_message(&js_value_to_string(&err));
            }
        });
    }

    async fn acquire_lock_and_promote(self: &Rc<Self>) -> Result<(), JsValue> {
        let global = js_sys::global();
        let navigator = Reflect::get(&global, &JsValue::from_str("navigator"))?;
        let locks = Reflect::get(&navigator, &JsValue::from_str("locks"))?;
        let request_value = Reflect::get(&locks, &JsValue::from_str("request"))?;
        let Some(request_fn) = request_value.dyn_ref::<Function>() else {
            self.promote_without_lock();
            return Ok(());
        };

        let options = Object::new();
        set_js_property(&options, "mode", &JsValue::from_str("exclusive"))?;
        let lock_id = format!("sqlite-database-{}", sanitize_identifier(&self.db_name));
        let state = Rc::clone(self);
        let handler = Closure::once(move |_lock: JsValue| -> Promise {
            state.on_lock_granted();
            Promise::new(&mut |_, _| {})
        });

        request_fn.call3(
            &locks,
            &JsValue::from_str(&lock_id),
            &options,
            handler.as_ref().unchecked_ref(),
        )?;
        handler.forget();
        Ok(())
    }

    fn promote_without_lock(self: &Rc<Self>) {
        self.on_lock_granted();
    }

    fn on_lock_granted(self: &Rc<Self>) {
        *self.role.borrow_mut() = LeadershipRole::Leader;
        self.mark_leader_known(self.worker_id.clone());

        let new_leader = ChannelMessage::NewLeader {
            leader_id: self.worker_id.clone(),
        };
        if let Err(err) = send_channel_message(&self.channel, &new_leader) {
            let _ = send_worker_error_message(&err);
        }
        if let Err(err) = self.spawn_db_worker() {
            let _ = send_worker_error_message(&js_value_to_string(&err));
        }
    }

    fn spawn_db_worker(self: &Rc<Self>) -> Result<(), JsValue> {
        let db_name_encoded =
            serde_json::to_string(&self.db_name).unwrap_or_else(|_| "\"unknown\"".to_string());
        let body_val = Reflect::get(
            &js_sys::global(),
            &JsValue::from_str("__SQLITE_EMBEDDED_WORKER"),
        )
        .map_err(|e| {
            JsValue::from_str(&format!(
                "Failed to read embedded worker source: {}",
                js_value_to_string(&e)
            ))
        })?;
        let body = body_val
            .as_string()
            .ok_or_else(|| JsValue::from_str("Embedded worker source is missing"))?;
        let preamble = format!(
            "self.__SQLITE_DB_ONLY = true;\nself.__SQLITE_DB_NAME = {};\nself.__SQLITE_FOLLOWER_TIMEOUT_MS = {};\n",
            db_name_encoded,
            self.follower_timeout_ms,
        );

        let parts = js_sys::Array::new();
        parts.push(&JsValue::from_str(&preamble));
        parts.push(&JsValue::from_str(&body));
        let options = BlobPropertyBag::new();
        options.set_type("application/javascript");
        let blob = Blob::new_with_str_sequence_and_options(&parts, &options)?;
        let url = Url::create_object_url_with_blob(&blob)?;

        let worker = Worker::new(&url)?;
        Url::revoke_object_url(&url)?;

        let state = Rc::clone(self);
        let handler = Closure::wrap(Box::new(move |event: MessageEvent| {
            state.handle_db_worker_event(event);
        }) as Box<dyn FnMut(MessageEvent)>);
        worker.set_onmessage(Some(handler.as_ref().unchecked_ref()));
        handler.forget();

        self.db_worker.borrow_mut().replace(worker);
        Ok(())
    }

    pub fn handle_db_worker_event(self: &Rc<Self>, event: MessageEvent) {
        let data = event.data();
        match serde_wasm_bindgen::from_value::<MainThreadMessage>(data.clone()) {
            Ok(MainThreadMessage::WorkerReady) => {
                *self.db_worker_ready.borrow_mut() = true;
                *self.leader_ready.borrow_mut() = true;
                let ready = ChannelMessage::LeaderReady {
                    leader_id: self.worker_id.clone(),
                };
                if let Err(err) = send_channel_message(&self.channel, &ready) {
                    let _ = send_worker_error_message(&err);
                }
                self.signal_ready_once();
            }
            Ok(MainThreadMessage::QueryResult {
                request_id,
                result,
                error,
            }) => {
                self.handle_db_query_result(request_id, result, error);
            }
            Err(_) => {
                if let Some(err) = parse_worker_error_payload(&data) {
                    self.handle_db_worker_failure(err);
                }
            }
        }
    }

    fn handle_db_worker_failure(self: &Rc<Self>, error: String) {
        *self.db_worker_ready.borrow_mut() = false;
        *self.leader_ready.borrow_mut() = false;
        *self.ready_signaled.borrow_mut() = false;
        if let Some(worker) = self.db_worker.borrow_mut().take() {
            worker.terminate();
        }
        let _ = send_worker_error_message(&error);
        let pending = self.db_pending.borrow_mut().drain().collect::<Vec<_>>();
        for (_, origin) in pending {
            self.fail_origin(origin, error.clone());
        }
        if let Err(err) = self.spawn_db_worker() {
            let _ = send_worker_error_message(&js_value_to_string(&err));
        }
    }

    pub fn handle_main_message(self: &Rc<Self>, msg: WorkerMessage) {
        match msg {
            WorkerMessage::ExecuteQuery {
                request_id,
                sql,
                params,
            } => match *self.role.borrow() {
                LeadershipRole::Leader => {
                    if !*self.db_worker_ready.borrow() {
                        let _ = send_query_result_to_main(
                            request_id,
                            Err(WORKER_ERROR_TYPE_INITIALIZATION_PENDING.to_string()),
                        );
                        return;
                    }
                    self.forward_query_to_db(DbRequestOrigin::Local { request_id }, sql, params);
                }
                LeadershipRole::Follower => {
                    if !*self.leader_ready.borrow() {
                        let _ = send_query_result_to_main(
                            request_id,
                            Err(WORKER_ERROR_TYPE_INITIALIZATION_PENDING.to_string()),
                        );
                        return;
                    }
                    let query_id = Uuid::new_v4().to_string();
                    self.follower_pending
                        .borrow_mut()
                        .insert(query_id.clone(), request_id);
                    let pending = Rc::clone(&self.follower_pending);
                    let timeout = self.follower_timeout_ms;
                    let timeout_query_id = query_id.clone();
                    spawn_local(async move {
                        sleep_ms(timeout.ceil() as i32).await;
                        if let Some(original) = pending.borrow_mut().remove(&timeout_query_id) {
                            let _ = send_query_result_to_main(
                                original,
                                Err("Query timeout".to_string()),
                            );
                        }
                    });
                    let request = ChannelMessage::QueryRequest {
                        query_id,
                        sql,
                        params,
                    };
                    if let Err(err) = send_channel_message(&self.channel, &request) {
                        let _ = send_worker_error_message(&err);
                    }
                }
            },
        }
    }

    fn handle_channel_message(self: &Rc<Self>, msg: ChannelMessage) {
        match msg {
            ChannelMessage::LeaderPing { requester_id: _ } => {
                if matches!(*self.role.borrow(), LeadershipRole::Leader) {
                    let response = if *self.db_worker_ready.borrow() {
                        ChannelMessage::LeaderReady {
                            leader_id: self.worker_id.clone(),
                        }
                    } else {
                        ChannelMessage::NewLeader {
                            leader_id: self.worker_id.clone(),
                        }
                    };
                    if let Err(err) = send_channel_message(&self.channel, &response) {
                        let _ = send_worker_error_message(&err);
                    }
                } else if *self.leader_ready.borrow() {
                    let leader_id = self
                        .leader_id
                        .borrow()
                        .clone()
                        .unwrap_or_else(|| self.worker_id.clone());
                    let response = ChannelMessage::LeaderReady { leader_id };
                    let _ = send_channel_message(&self.channel, &response);
                }
            }
            ChannelMessage::NewLeader { leader_id } => {
                self.mark_leader_known(leader_id);
            }
            ChannelMessage::LeaderReady { leader_id } => {
                self.mark_leader_known(leader_id);
                *self.leader_ready.borrow_mut() = true;
                self.signal_ready_once();
            }
            ChannelMessage::QueryRequest {
                query_id,
                sql,
                params,
            } => {
                if matches!(*self.role.borrow(), LeadershipRole::Leader) {
                    if !*self.db_worker_ready.borrow() {
                        let _ = send_channel_message(
                            &self.channel,
                            &ChannelMessage::QueryResponse {
                                query_id,
                                result: None,
                                error: Some(WORKER_ERROR_TYPE_INITIALIZATION_PENDING.to_string()),
                            },
                        );
                        return;
                    }
                    self.forward_query_to_db(DbRequestOrigin::Forwarded { query_id }, sql, params);
                }
            }
            ChannelMessage::QueryResponse {
                query_id,
                result,
                error,
            } => {
                if let Some(request_id) = self.follower_pending.borrow_mut().remove(&query_id) {
                    let outcome = match (result, error) {
                        (Some(res), _) => Ok(res),
                        (_, Some(err)) => Err(err),
                        _ => Err("Unknown query response".to_string()),
                    };
                    let _ = send_query_result_to_main(request_id, outcome);
                }
            }
        }
    }

    fn forward_query_to_db(
        self: &Rc<Self>,
        origin: DbRequestOrigin,
        sql: String,
        params: Option<Vec<serde_json::Value>>,
    ) {
        let worker = {
            let borrow = self.db_worker.borrow();
            let Some(worker) = borrow.as_ref() else {
                match origin {
                    DbRequestOrigin::Local { request_id } => {
                        let _ = send_query_result_to_main(
                            request_id,
                            Err(WORKER_ERROR_TYPE_INITIALIZATION_PENDING.to_string()),
                        );
                    }
                    DbRequestOrigin::Forwarded { query_id } => {
                        let _ = send_channel_message(
                            &self.channel,
                            &ChannelMessage::QueryResponse {
                                query_id,
                                result: None,
                                error: Some(WORKER_ERROR_TYPE_INITIALIZATION_PENDING.to_string()),
                            },
                        );
                    }
                }
                return;
            };
            worker.clone()
        };

        let db_request_id = {
            let mut next = self.next_db_request_id.borrow_mut();
            let id = *next;
            *next = next.wrapping_add(1).max(1);
            id
        };
        self.db_pending.borrow_mut().insert(db_request_id, origin);

        let msg = WorkerMessage::ExecuteQuery {
            request_id: db_request_id,
            sql,
            params,
        };
        match serde_wasm_bindgen::to_value(&msg) {
            Ok(val) => {
                if let Err(err) = worker.post_message(&val) {
                    let _ = send_worker_error_message(&js_value_to_string(&err));
                    if let Some(origin) = self.db_pending.borrow_mut().remove(&db_request_id) {
                        self.fail_origin(
                            origin,
                            "Failed to dispatch query to DB worker".to_string(),
                        );
                    }
                }
            }
            Err(err) => {
                let _ = send_worker_error_message(&format!("{err:?}"));
                if let Some(origin) = self.db_pending.borrow_mut().remove(&db_request_id) {
                    self.fail_origin(origin, "Failed to serialize query".to_string());
                }
            }
        }
    }

    fn fail_origin(&self, origin: DbRequestOrigin, error: String) {
        match origin {
            DbRequestOrigin::Local { request_id } => {
                let _ = send_query_result_to_main(request_id, Err(error));
            }
            DbRequestOrigin::Forwarded { query_id } => {
                let _ = send_channel_message(
                    &self.channel,
                    &ChannelMessage::QueryResponse {
                        query_id,
                        result: None,
                        error: Some(error),
                    },
                );
            }
        }
    }

    fn handle_db_query_result(
        self: &Rc<Self>,
        db_request_id: u32,
        result: Option<String>,
        error: Option<WorkerErrorPayload>,
    ) {
        let Some(origin) = self.db_pending.borrow_mut().remove(&db_request_id) else {
            return;
        };
        let outcome = match (result, error) {
            (Some(res), _) => Ok(res),
            (_, Some(err)) => Err(error_payload_to_string(&err)),
            _ => Err("Invalid response from DB worker".to_string()),
        };
        match origin {
            DbRequestOrigin::Local { request_id } => {
                let _ = send_query_result_to_main(request_id, outcome);
            }
            DbRequestOrigin::Forwarded { query_id } => match outcome {
                Ok(res) => {
                    let _ = send_channel_message(
                        &self.channel,
                        &ChannelMessage::QueryResponse {
                            query_id,
                            result: Some(res),
                            error: None,
                        },
                    );
                }
                Err(err) => {
                    let _ = send_channel_message(
                        &self.channel,
                        &ChannelMessage::QueryResponse {
                            query_id,
                            result: None,
                            error: Some(err),
                        },
                    );
                }
            },
        }
    }

    fn mark_leader_known(&self, leader_id: String) {
        *self.leader_id.borrow_mut() = Some(leader_id);
    }

    fn signal_ready_once(&self) {
        if *self.ready_signaled.borrow() {
            return;
        }
        *self.ready_signaled.borrow_mut() = true;
        if let Err(err) = send_worker_ready_message() {
            let _ = send_worker_error_message(&err);
        }
    }
}

impl DbWorkerState {
    pub fn new(config: WorkerConfig) -> Rc<Self> {
        Rc::new(DbWorkerState {
            db: Rc::new(RefCell::new(None)),
            db_name: config.db_name,
            db_queue: Rc::new(RefCell::new(VecDeque::new())),
            db_processing: Rc::new(Cell::new(false)),
        })
    }

    pub fn start(self: &Rc<Self>) {
        let state = Rc::clone(self);
        spawn_local(async move {
            match SQLiteDatabase::initialize_opfs(&state.db_name).await {
                Ok(db) => {
                    *state.db.borrow_mut() = Some(db);
                    let _ = send_worker_ready_message();
                }
                Err(err) => {
                    let _ = send_worker_error_message(&js_value_to_string(&err));
                }
            }
        });
    }

    pub fn handle_message(self: &Rc<Self>, msg: WorkerMessage) {
        match msg {
            WorkerMessage::ExecuteQuery {
                request_id,
                sql,
                params,
            } => {
                self.enqueue_query(request_id, sql, params);
            }
        }
    }

    fn enqueue_query(
        self: &Rc<Self>,
        request_id: u32,
        sql: String,
        params: Option<Vec<serde_json::Value>>,
    ) {
        self.db_queue.borrow_mut().push_back(DbJob {
            request_id,
            sql,
            params,
        });
        self.start_queue_processor();
    }

    fn start_queue_processor(self: &Rc<Self>) {
        if self.db_processing.replace(true) {
            return;
        }
        let state = Rc::clone(self);
        spawn_local(async move {
            loop {
                let job = {
                    let mut queue = state.db_queue.borrow_mut();
                    queue.pop_front()
                };
                let Some(job) = job else { break };
                let db = Rc::clone(&state.db);
                let result = exec_on_db(db, job.sql, job.params).await;
                match make_query_result_message(job.request_id, result) {
                    Ok(resp) => {
                        let _ = post_worker_message(&resp);
                    }
                    Err(err) => {
                        let _ = send_worker_error(err);
                    }
                }
            }
            state.db_processing.set(false);
            if !state.db_queue.borrow().is_empty() {
                state.start_queue_processor();
            }
        });
    }
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

fn error_payload_to_string(payload: &WorkerErrorPayload) -> String {
    payload
        .message
        .clone()
        .unwrap_or_else(|| payload.error_type.clone())
}

pub fn send_worker_ready_message() -> Result<(), String> {
    let message = js_sys::Object::new();
    set_js_property(&message, "type", &JsValue::from_str("worker-ready"))
        .map_err(|err| js_value_to_string(&err))?;
    post_worker_message(&message)
}

pub fn send_worker_error_message(error: &str) -> Result<(), String> {
    let message = js_sys::Object::new();
    set_js_property(&message, "type", &JsValue::from_str("worker-error"))
        .map_err(|err| js_value_to_string(&err))?;
    set_js_property(&message, "error", &JsValue::from_str(error))
        .map_err(|err| js_value_to_string(&err))?;
    post_worker_message(&message)
}

pub fn post_worker_message(obj: &js_sys::Object) -> Result<(), String> {
    let global = js_sys::global();
    let scope: DedicatedWorkerGlobalScope = global
        .dyn_into()
        .map_err(|_| "Failed to access worker scope".to_string())?;
    scope
        .post_message(obj.as_ref())
        .map_err(|err| js_value_to_string(&err))
}

pub fn send_worker_error(err: JsValue) -> Result<(), JsValue> {
    let message = js_value_to_string(&err);
    send_worker_error_message(&message).map_err(|post_err| {
        JsValue::from_str(&format!(
            "Failed to deliver worker error '{message}': {}",
            post_err
        ))
    })
}

fn parse_worker_error_payload(data: &JsValue) -> Option<String> {
    let msg_type = Reflect::get(data, &JsValue::from_str("type"))
        .ok()
        .and_then(|val| val.as_string())?;
    if msg_type != "worker-error" {
        return None;
    }
    Reflect::get(data, &JsValue::from_str("error"))
        .ok()
        .and_then(|val| {
            if val.is_undefined() || val.is_null() {
                None
            } else {
                Some(js_value_to_string(&val))
            }
        })
        .or_else(|| Some("Unknown worker error".to_string()))
}

fn make_structured_error(err: &str) -> Result<JsValue, JsValue> {
    let error_object = js_sys::Object::new();
    let error_type = if err == WORKER_ERROR_TYPE_INITIALIZATION_PENDING {
        WORKER_ERROR_TYPE_INITIALIZATION_PENDING
    } else {
        crate::messages::WORKER_ERROR_TYPE_GENERIC
    };
    set_js_property(
        error_object.as_ref(),
        "type",
        &JsValue::from_str(error_type),
    )?;
    set_js_property(error_object.as_ref(), "message", &JsValue::from_str(err))?;
    Ok(error_object.into())
}

pub fn make_query_result_message(
    request_id: u32,
    result: Result<String, String>,
) -> Result<js_sys::Object, JsValue> {
    let response = js_sys::Object::new();
    set_js_property(&response, "type", &JsValue::from_str("query-result"))?;
    set_js_property(
        &response,
        "requestId",
        &JsValue::from_f64(request_id as f64),
    )?;
    match result {
        Ok(res) => {
            set_js_property(&response, "result", &JsValue::from_str(&res))?;
            set_js_property(&response, "error", &JsValue::NULL)?;
        }
        Err(err) => {
            set_js_property(&response, "result", &JsValue::NULL)?;
            let error_value = make_structured_error(&err)?;
            set_js_property(&response, "error", &error_value)?;
        }
    }
    Ok(response)
}

pub fn send_query_result_to_main(
    request_id: u32,
    result: Result<String, String>,
) -> Result<(), JsValue> {
    let message = make_query_result_message(request_id, result)?;
    post_worker_message(&message).map_err(|err| JsValue::from_str(&err))
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
        None => Err(WORKER_ERROR_TYPE_INITIALIZATION_PENDING.to_string()),
    };
    result
}

pub async fn sleep_ms(ms: i32) {
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
            let _ = resolve.call0(&JsValue::NULL);
        }

        closure.forget();
    });
    let _ = JsFuture::from(promise).await;
}

#[cfg(all(test, target_family = "wasm"))]
mod tests {
    use super::*;
    use crate::messages::ChannelMessage;
    use crate::util::sanitize_identifier;
    use std::cell::RefCell;

    wasm_bindgen_test_configure!(run_in_browser);

    fn set_global_str(key: &str, value: &str) {
        let _ = Reflect::set(
            &js_sys::global(),
            &JsValue::from_str(key),
            &JsValue::from_str(value),
        );
    }

    fn set_global_num(key: &str, value: f64) {
        let _ = Reflect::set(
            &js_sys::global(),
            &JsValue::from_str(key),
            &JsValue::from_f64(value),
        );
    }

    #[wasm_bindgen_test(async)]
    async fn coordinator_broadcasts_leader_and_ready() {
        set_global_str("__SQLITE_DB_NAME", "testdb-coordinator");
        set_global_num("__SQLITE_FOLLOWER_TIMEOUT_MS", 100.0);
        set_global_str(
            "__SQLITE_EMBEDDED_WORKER",
            "self.postMessage({type:'worker-ready'}); self.onmessage = ev => { const d = ev.data || {}; if (d.type === 'execute-query') { self.postMessage({type:'query-result', requestId:d.requestId, result:'{\"ok\":true}', error:null}); } };",
        );

        let cfg = worker_config_from_global().expect("config");
        let state = CoordinatorState::new(cfg).expect("state");

        let channel_name = format!("sqlite-queries-{}", sanitize_identifier(&state.db_name));
        let observer = BroadcastChannel::new(&channel_name).expect("observer channel");

        let received: Rc<RefCell<Vec<ChannelMessage>>> = Rc::new(RefCell::new(Vec::new()));
        let recv_clone = Rc::clone(&received);
        let listener = Closure::wrap(Box::new(move |event: MessageEvent| {
            if let Ok(msg) = serde_wasm_bindgen::from_value::<ChannelMessage>(event.data()) {
                recv_clone.borrow_mut().push(msg);
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        observer.set_onmessage(Some(listener.as_ref().unchecked_ref()));
        listener.forget();

        state.on_lock_granted();
        sleep_ms(50).await;

        let msgs = received.borrow();
        assert!(
            msgs.iter()
                .any(|m| matches!(m, ChannelMessage::NewLeader { .. })),
            "should announce new-leader"
        );
        assert!(
            msgs.iter()
                .any(|m| matches!(m, ChannelMessage::LeaderReady { .. })),
            "should announce leader-ready"
        );
    }

    #[wasm_bindgen_test(async)]
    async fn leader_ping_responds_based_on_db_readiness() {
        set_global_str("__SQLITE_DB_NAME", "testdb-ping");
        set_global_num("__SQLITE_FOLLOWER_TIMEOUT_MS", 50.0);
        set_global_str("__SQLITE_EMBEDDED_WORKER", "");

        let cfg = worker_config_from_global().expect("config");
        let state = CoordinatorState::new(cfg).expect("state");
        *state.role.borrow_mut() = LeadershipRole::Leader;

        let channel_name = format!("sqlite-queries-{}", sanitize_identifier(&state.db_name));
        let observer = BroadcastChannel::new(&channel_name).expect("observer channel");

        let received: Rc<RefCell<Vec<ChannelMessage>>> = Rc::new(RefCell::new(Vec::new()));
        let recv_clone = Rc::clone(&received);
        let listener = Closure::wrap(Box::new(move |event: MessageEvent| {
            if let Ok(msg) = serde_wasm_bindgen::from_value::<ChannelMessage>(event.data()) {
                recv_clone.borrow_mut().push(msg);
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        observer.set_onmessage(Some(listener.as_ref().unchecked_ref()));
        listener.forget();

        *state.db_worker_ready.borrow_mut() = false;
        state.handle_channel_message(ChannelMessage::LeaderPing {
            requester_id: "follower".to_string(),
        });
        sleep_ms(10).await;
        assert!(
            received
                .borrow()
                .iter()
                .any(|m| matches!(m, ChannelMessage::NewLeader { .. })),
            "should answer with new-leader when DB not ready"
        );

        received.borrow_mut().clear();
        *state.db_worker_ready.borrow_mut() = true;
        state.handle_channel_message(ChannelMessage::LeaderPing {
            requester_id: "follower".to_string(),
        });
        sleep_ms(10).await;
        assert!(
            received
                .borrow()
                .iter()
                .any(|m| matches!(m, ChannelMessage::LeaderReady { .. })),
            "should answer with leader-ready once DB is ready"
        );
    }
}
