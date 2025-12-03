use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

use crate::coordination::{
    send_worker_error, worker_config_from_global, CoordinatorState, DbWorkerState, WorkerConfig,
};
use crate::messages::WorkerMessage;

enum WorkerRuntime {
    Coordinator(Rc<CoordinatorState>),
    DbOnly(Rc<DbWorkerState>),
}

thread_local! {
    static RUNTIME: RefCell<Option<WorkerRuntime>> = const { RefCell::new(None) };
}

fn is_db_only_mode() -> bool {
    let global = js_sys::global();
    js_sys::Reflect::get(&global, &JsValue::from_str("__SQLITE_DB_ONLY"))
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn install_main_thread_handler() {
    let global = js_sys::global();
    let worker_scope: DedicatedWorkerGlobalScope = global.unchecked_into();
    let onmessage = Closure::wrap(Box::new(move |event: MessageEvent| {
        match serde_wasm_bindgen::from_value::<WorkerMessage>(event.data()) {
            Ok(msg) => {
                RUNTIME.with(|runtime| {
                    if let Some(inner) = runtime.borrow().as_ref() {
                        match inner {
                            WorkerRuntime::Coordinator(state) => state.handle_main_message(msg),
                            WorkerRuntime::DbOnly(db) => db.handle_message(msg),
                        }
                    }
                });
            }
            Err(err) => {
                let _ = send_worker_error(JsValue::from_str(&format!(
                    "Invalid worker message: {err:?}"
                )));
            }
        }
    }) as Box<dyn FnMut(MessageEvent)>);

    worker_scope.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();
}

fn start_coordinator_runtime(config: WorkerConfig) -> Result<(), JsValue> {
    let state = CoordinatorState::new(config)?;
    state.setup_channel_listener()?;
    state.start_leader_probe();
    state.try_become_leader();

    RUNTIME.with(|runtime| {
        *runtime.borrow_mut() = Some(WorkerRuntime::Coordinator(Rc::clone(&state)));
    });

    install_main_thread_handler();
    Ok(())
}

fn start_db_only_runtime(config: WorkerConfig) -> Result<(), JsValue> {
    let state = DbWorkerState::new(config);
    state.start();
    RUNTIME.with(|runtime| {
        *runtime.borrow_mut() = Some(WorkerRuntime::DbOnly(Rc::clone(&state)));
    });
    install_main_thread_handler();
    Ok(())
}

/// Entry point for the worker - called from the blob
pub fn main() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let config = worker_config_from_global()?;

    if is_db_only_mode() {
        start_db_only_runtime(config)
    } else {
        start_coordinator_runtime(config)
    }
}

#[cfg(all(test, target_family = "wasm"))]
mod tests {
    use super::*;
    use js_sys::Reflect;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn db_only_mode_defaults_to_false() {
        let _ = Reflect::delete_property(&js_sys::global(), &JsValue::from_str("__SQLITE_DB_ONLY"));
        assert!(!is_db_only_mode());
    }

    #[wasm_bindgen_test]
    fn db_only_mode_reads_flag() {
        Reflect::set(
            &js_sys::global(),
            &JsValue::from_str("__SQLITE_DB_ONLY"),
            &JsValue::TRUE,
        )
        .unwrap();
        assert!(is_db_only_mode());
    }
}
