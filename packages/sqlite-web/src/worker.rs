use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ready::ReadySignal;
use crate::utils::describe_js_value;
use js_sys::{Array, Function, Reflect};
use serde::Deserialize;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_utils::prelude::serde_wasm_bindgen;
use web_sys::{Blob, BlobPropertyBag, MessageEvent, Url, Worker};

pub(crate) fn create_worker_from_code(worker_code: &str) -> Result<Worker, JsValue> {
    let blob_parts = Array::new();
    blob_parts.push(&JsValue::from_str(worker_code));

    let blob_options = BlobPropertyBag::new();
    blob_options.set_type("application/javascript");

    let blob = Blob::new_with_str_sequence_and_options(&blob_parts, &blob_options)?;
    let worker_url = Url::create_object_url_with_blob(&blob)?;
    let worker_res = Worker::new(&worker_url);
    Url::revoke_object_url(&worker_url)?;
    worker_res
}

pub(crate) fn install_onmessage_handler(
    worker: &Worker,
    pending_queries: Rc<RefCell<HashMap<u32, (Function, Function)>>>,
    ready_signal: ReadySignal,
) {
    let pending_queries_clone = Rc::clone(&pending_queries);
    let ready_signal_clone = ready_signal.clone();
    let onmessage = Closure::wrap(Box::new(move |event: MessageEvent| {
        let data = event.data();
        if handle_worker_control_message(&data, &ready_signal_clone) {
            return;
        }
        handle_query_result_message(&data, &pending_queries_clone);
    }) as Box<dyn FnMut(MessageEvent)>);

    worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum WorkerControlMessage {
    #[serde(rename = "worker-ready")]
    Ready,
    #[serde(rename = "worker-error")]
    Error,
}

pub(crate) fn handle_worker_control_message(data: &JsValue, ready_signal: &ReadySignal) -> bool {
    match serde_wasm_bindgen::from_value::<WorkerControlMessage>(data.clone()) {
        Ok(WorkerControlMessage::Ready) => {
            ready_signal.mark_ready();
            true
        }
        Ok(WorkerControlMessage::Error) => {
            let reason = Reflect::get(data, &JsValue::from_str("error"))
                .ok()
                .filter(|val| !val.is_null() && !val.is_undefined())
                .map(|val| describe_js_value(&val))
                .unwrap_or_else(|| "Unknown worker error".to_string());
            ready_signal.mark_failed(reason);
            true
        }
        Err(_) => false,
    }
}

fn handle_query_result_message(
    data: &JsValue,
    pending_queries: &Rc<RefCell<HashMap<u32, (Function, Function)>>>,
) {
    let msg_type = Reflect::get(data, &JsValue::from_str("type"))
        .ok()
        .and_then(|obj| obj.as_string());

    let Some(msg_type) = msg_type else { return };
    if msg_type != "query-result" {
        return;
    }

    let req_id_js = Reflect::get(data, &JsValue::from_str("requestId")).ok();
    let req_id = req_id_js.and_then(|v| v.as_f64()).map(|n| n as u32);
    let Some(request_id) = req_id else { return };
    let entry = pending_queries.borrow_mut().remove(&request_id);
    let Some((resolve, reject)) = entry else {
        return;
    };

    let error = Reflect::get(data, &JsValue::from_str("error"))
        .ok()
        .filter(|e| !e.is_null() && !e.is_undefined());

    if let Some(error) = error {
        let error_str = error.as_string().unwrap_or_else(|| format!("{error:?}"));
        let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(&error_str));
        return;
    }

    if let Some(result) = Reflect::get(data, &JsValue::from_str("result"))
        .ok()
        .filter(|r| !r.is_null() && !r.is_undefined())
    {
        let result_str = result.as_string().unwrap_or_else(|| format!("{result:?}"));
        let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str(&result_str));
    }
}

#[cfg(all(test, target_family = "wasm"))]
mod tests {
    use super::*;
    use crate::ready::InitializationState;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    fn recorder_function() -> (js_sys::Function, Rc<RefCell<Vec<String>>>) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let calls_clone = Rc::clone(&calls);
        let closure = Closure::wrap(Box::new(move |value: JsValue| {
            calls_clone
                .borrow_mut()
                .push(value.as_string().unwrap_or_else(|| format!("{value:?}")));
        }) as Box<dyn FnMut(JsValue)>);
        let func: js_sys::Function = closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
        closure.forget();
        (func, calls)
    }

    #[wasm_bindgen_test]
    fn worker_control_message_marks_ready() {
        let signal = ReadySignal::new();
        let msg = js_sys::Object::new();
        let _ = js_sys::Reflect::set(
            &msg,
            &JsValue::from_str("type"),
            &JsValue::from_str("worker-ready"),
        );

        let handled = handle_worker_control_message(&msg.into(), &signal);
        assert!(handled);
        assert!(matches!(signal.current_state(), InitializationState::Ready));
    }

    #[wasm_bindgen_test]
    fn worker_control_message_marks_failed_with_reason() {
        let signal = ReadySignal::new();
        let msg = js_sys::Object::new();
        let _ = js_sys::Reflect::set(
            &msg,
            &JsValue::from_str("type"),
            &JsValue::from_str("worker-error"),
        );
        let _ = js_sys::Reflect::set(
            &msg,
            &JsValue::from_str("error"),
            &JsValue::from_str("boom"),
        );

        let handled = handle_worker_control_message(&msg.into(), &signal);
        assert!(handled);
        match signal.current_state() {
            InitializationState::Failed(reason) => assert_eq!(reason, "boom"),
            other => panic!("expected Failed state, got {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn query_result_message_resolves_registered_pending_call() {
        let (resolve_fn, resolve_calls) = recorder_function();
        let (reject_fn, reject_calls) = recorder_function();
        let pending_queries = Rc::new(RefCell::new(HashMap::new()));
        pending_queries
            .borrow_mut()
            .insert(7, (resolve_fn, reject_fn));

        let msg = js_sys::Object::new();
        let _ = js_sys::Reflect::set(
            &msg,
            &JsValue::from_str("type"),
            &JsValue::from_str("query-result"),
        );
        let _ = js_sys::Reflect::set(
            &msg,
            &JsValue::from_str("requestId"),
            &JsValue::from_f64(7.0),
        );
        let _ = js_sys::Reflect::set(&msg, &JsValue::from_str("result"), &JsValue::from_str("ok"));

        let msg: JsValue = msg.into();
        handle_query_result_message(&msg, &pending_queries);

        {
            let calls = resolve_calls.borrow();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0], "ok");
        }
        assert!(reject_calls.borrow().is_empty());
        assert!(pending_queries.borrow().is_empty());
    }

    #[wasm_bindgen_test]
    fn query_result_message_rejects_with_error_payload() {
        let (resolve_fn, resolve_calls) = recorder_function();
        let (reject_fn, reject_calls) = recorder_function();
        let pending_queries = Rc::new(RefCell::new(HashMap::new()));
        pending_queries
            .borrow_mut()
            .insert(3, (resolve_fn, reject_fn));

        let msg = js_sys::Object::new();
        let _ = js_sys::Reflect::set(
            &msg,
            &JsValue::from_str("type"),
            &JsValue::from_str("query-result"),
        );
        let _ = js_sys::Reflect::set(
            &msg,
            &JsValue::from_str("requestId"),
            &JsValue::from_f64(3.0),
        );
        let _ = js_sys::Reflect::set(
            &msg,
            &JsValue::from_str("error"),
            &JsValue::from_str("nope"),
        );

        let msg: JsValue = msg.into();
        handle_query_result_message(&msg, &pending_queries);

        assert!(resolve_calls.borrow().is_empty());
        {
            let calls = reject_calls.borrow();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0], "nope");
        }
        assert!(pending_queries.borrow().is_empty());
    }
}
