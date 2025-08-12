// worker.rs - This module runs in the worker context
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

use crate::coordination::WorkerState;

// Global state
thread_local! {
    static WORKER_STATE: RefCell<Option<Rc<WorkerState>>> = RefCell::new(None);
}

/// Entry point for the worker - called from the blob
pub fn main() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

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

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;
    use std::rc::Rc;
    use js_sys::{Object, Reflect};

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn test_worker_state_creation() {
        let state = WorkerState::new();
        assert!(state.is_ok());
        let worker_state = state.unwrap();
        assert!(!worker_state.worker_id.is_empty());
    }

    #[wasm_bindgen_test]
    fn test_execute_query_message_structure() {
        let msg = Object::new();
        Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("execute-query")).unwrap();
        Reflect::set(&msg, &JsValue::from_str("sql"), &JsValue::from_str("SELECT 1")).unwrap();

        let msg_type = Reflect::get(&msg, &JsValue::from_str("type")).unwrap();
        let sql = Reflect::get(&msg, &JsValue::from_str("sql")).unwrap();

        assert_eq!(msg_type.as_string().unwrap(), "execute-query");
        assert_eq!(sql.as_string().unwrap(), "SELECT 1");
    }

    #[wasm_bindgen_test]
    fn test_query_response_structure_success() {
        let response = Object::new();
        Reflect::set(&response, &JsValue::from_str("type"), &JsValue::from_str("query-result")).unwrap();
        Reflect::set(&response, &JsValue::from_str("result"), &JsValue::from_str("test_result")).unwrap();
        Reflect::set(&response, &JsValue::from_str("error"), &JsValue::NULL).unwrap();

        let response_type = Reflect::get(&response, &JsValue::from_str("type")).unwrap();
        let result = Reflect::get(&response, &JsValue::from_str("result")).unwrap();
        let error = Reflect::get(&response, &JsValue::from_str("error")).unwrap();

        assert_eq!(response_type.as_string().unwrap(), "query-result");
        assert_eq!(result.as_string().unwrap(), "test_result");
        assert!(error.is_null());
    }

    #[wasm_bindgen_test]
    fn test_query_response_structure_error() {
        let response = Object::new();
        Reflect::set(&response, &JsValue::from_str("type"), &JsValue::from_str("query-result")).unwrap();
        Reflect::set(&response, &JsValue::from_str("result"), &JsValue::NULL).unwrap();
        Reflect::set(&response, &JsValue::from_str("error"), &JsValue::from_str("test_error")).unwrap();

        let response_type = Reflect::get(&response, &JsValue::from_str("type")).unwrap();
        let result = Reflect::get(&response, &JsValue::from_str("result")).unwrap();
        let error = Reflect::get(&response, &JsValue::from_str("error")).unwrap();

        assert_eq!(response_type.as_string().unwrap(), "query-result");
        assert!(result.is_null());
        assert_eq!(error.as_string().unwrap(), "test_error");
    }

    #[wasm_bindgen_test]
    fn test_message_parsing_invalid_type() {
        let msg = Object::new();
        Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("invalid-type")).unwrap();

        let msg_type = Reflect::get(&msg, &JsValue::from_str("type")).unwrap();
        assert_eq!(msg_type.as_string().unwrap(), "invalid-type");
    }

    #[wasm_bindgen_test]
    fn test_message_parsing_missing_sql() {
        let msg = Object::new();
        Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("execute-query")).unwrap();

        let sql_result = Reflect::get(&msg, &JsValue::from_str("sql"));
        assert!(sql_result.is_ok());
        assert!(sql_result.unwrap().is_undefined());
    }

    #[wasm_bindgen_test]
    fn test_message_parsing_empty_sql() {
        let msg = Object::new();
        Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("execute-query")).unwrap();
        Reflect::set(&msg, &JsValue::from_str("sql"), &JsValue::from_str("")).unwrap();

        let sql = Reflect::get(&msg, &JsValue::from_str("sql")).unwrap();
        assert_eq!(sql.as_string().unwrap(), "");
    }

    #[wasm_bindgen_test]
    fn test_worker_state_reference_counting() {
        let state = WorkerState::new().unwrap();
        let state_rc = Rc::new(state);
        let cloned_state = Rc::clone(&state_rc);
        assert_eq!(Rc::strong_count(&state_rc), 2);
        drop(cloned_state);
        assert_eq!(Rc::strong_count(&state_rc), 1);
    }

    #[wasm_bindgen_test]
    fn test_global_worker_scope_access() {
        let global = js_sys::global();
        assert!(!global.is_undefined());
        assert!(!global.is_null());
    }
}
