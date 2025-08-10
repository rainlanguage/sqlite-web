// worker.rs - This module runs in the worker context
use js_sys::Reflect;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

use crate::coordination::WorkerState;
use crate::messages::{ChannelMessage, MainThreadMessage, PendingQuery, WorkerMessage};

// Global state
thread_local! {
    static WORKER_STATE: RefCell<Option<Rc<WorkerState>>> = RefCell::new(None);
}

/// Entry point for the worker - called from the blob
#[wasm_bindgen]
pub fn worker_main() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    web_sys::console::log_1(&"[Worker] Starting worker_main...".into());

    let state = Rc::new(WorkerState::new()?);

    state.setup_channel_listener();

    let state_clone = Rc::clone(&state);
    spawn_local(async move {
        state_clone.attempt_leadership().await;
    });

    WORKER_STATE.with(|s| {
        *s.borrow_mut() = Some(Rc::clone(&state));
    });

    // Setup message handler from main thread
    let global = js_sys::global();
    let worker_scope: DedicatedWorkerGlobalScope = global.unchecked_into();

    let onmessage = Closure::wrap(Box::new(move |event: MessageEvent| {
        let data = event.data();

        // Parse JavaScript message directly
        if let Ok(msg_type) = js_sys::Reflect::get(&data, &JsValue::from_str("type")) {
            if let Some(type_str) = msg_type.as_string() {
                if type_str == "execute-query" {
                    if let Ok(sql_val) = js_sys::Reflect::get(&data, &JsValue::from_str("sql")) {
                        if let Some(sql) = sql_val.as_string() {
                            WORKER_STATE.with(|s| {
                                if let Some(state) = s.borrow().as_ref() {
                                    let state = Rc::clone(state);
                                    spawn_local(async move {
                                        let result = state.execute_query(sql).await;

                                        // Send response as plain JavaScript object
                                        let response = js_sys::Object::new();
                                        js_sys::Reflect::set(
                                            &response,
                                            &JsValue::from_str("type"),
                                            &JsValue::from_str("query-result"),
                                        )
                                        .unwrap();

                                        match result {
                                            Ok(res) => {
                                                js_sys::Reflect::set(
                                                    &response,
                                                    &JsValue::from_str("result"),
                                                    &JsValue::from_str(&res),
                                                )
                                                .unwrap();
                                                js_sys::Reflect::set(
                                                    &response,
                                                    &JsValue::from_str("error"),
                                                    &JsValue::NULL,
                                                )
                                                .unwrap();
                                            }
                                            Err(err) => {
                                                js_sys::Reflect::set(
                                                    &response,
                                                    &JsValue::from_str("result"),
                                                    &JsValue::NULL,
                                                )
                                                .unwrap();
                                                js_sys::Reflect::set(
                                                    &response,
                                                    &JsValue::from_str("error"),
                                                    &JsValue::from_str(&err),
                                                )
                                                .unwrap();
                                            }
                                        };

                                        let global = js_sys::global();
                                        let worker_scope: DedicatedWorkerGlobalScope =
                                            global.unchecked_into();
                                        let _ = worker_scope.post_message(&response);
                                    });
                                }
                            });
                        }
                    }
                }
            }
        }
    }) as Box<dyn FnMut(MessageEvent)>);

    worker_scope.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();

    Ok(())
}
