use std::cell::RefCell;
use std::rc::Rc;

use js_sys::{Function, Promise};
use wasm_bindgen::prelude::*;

use crate::errors::SQLiteWasmDatabaseError;

#[derive(Debug, Clone)]
pub(crate) enum InitializationState {
    Pending,
    Ready,
    Failed(String),
}

#[derive(Clone)]
pub(crate) struct ReadySignal {
    state: Rc<RefCell<InitializationState>>,
    resolve: Rc<RefCell<Option<Function>>>,
    reject: Rc<RefCell<Option<Function>>>,
    promise: Rc<RefCell<Option<Promise>>>,
}

impl ReadySignal {
    pub(crate) fn new() -> Self {
        let state = Rc::new(RefCell::new(InitializationState::Pending));
        let resolve = Rc::new(RefCell::new(None));
        let reject = Rc::new(RefCell::new(None));
        let promise = Rc::new(RefCell::new(None));
        {
            let ready_promise = create_ready_promise(&resolve, &reject);
            promise.borrow_mut().replace(ready_promise);
        }
        Self {
            state,
            resolve,
            reject,
            promise,
        }
    }

    pub(crate) fn current_state(&self) -> InitializationState {
        self.state.borrow().clone()
    }

    pub(crate) fn wait_promise(&self) -> Result<Promise, SQLiteWasmDatabaseError> {
        self.promise.borrow().as_ref().cloned().ok_or_else(|| {
            SQLiteWasmDatabaseError::InitializationFailed(
                "Worker readiness promise missing".to_string(),
            )
        })
    }

    pub(crate) fn mark_ready(&self) {
        let mut transitioned = false;
        {
            let mut state = self.state.borrow_mut();
            match *state {
                InitializationState::Failed(_) => return,
                InitializationState::Ready => {}
                _ => {
                    *state = InitializationState::Ready;
                    transitioned = true;
                }
            }
        }
        if !transitioned {
            return;
        }
        if let Some(resolve) = self.resolve.borrow_mut().take() {
            let _ = resolve.call0(&JsValue::NULL);
        }
        self.reject.borrow_mut().take();
        self.promise.borrow_mut().take();
    }

    pub(crate) fn mark_failed(&self, reason: String) {
        {
            let mut state = self.state.borrow_mut();
            if !matches!(&*state, InitializationState::Failed(existing) if existing == &reason) {
                *state = InitializationState::Failed(reason.clone());
            }
        }
        self.resolve.borrow_mut().take();
        if let Some(reject) = self.reject.borrow_mut().take() {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(&reason));
        }
        self.promise.borrow_mut().take();
    }

    pub(crate) fn reset(&self) {
        self.resolve.borrow_mut().take();
        if let Some(reject) = self.reject.borrow_mut().take() {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("Ready signal reset"));
        }
        self.promise.borrow_mut().take();
        *self.state.borrow_mut() = InitializationState::Pending;
        let ready_promise = create_ready_promise(&self.resolve, &self.reject);
        self.promise.borrow_mut().replace(ready_promise);
    }
}

fn create_ready_promise(
    resolve_cell: &Rc<RefCell<Option<Function>>>,
    reject_cell: &Rc<RefCell<Option<Function>>>,
) -> Promise {
    let resolve_clone = Rc::clone(resolve_cell);
    let reject_clone = Rc::clone(reject_cell);
    Promise::new(&mut move |resolve, reject| {
        resolve_clone.borrow_mut().replace(resolve);
        reject_clone.borrow_mut().replace(reject);
    })
}

#[cfg(all(test, target_family = "wasm"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test(async)]
    async fn ready_signal_resolves_when_marked_ready() {
        let signal = ReadySignal::new();
        let promise = signal.wait_promise().expect("promise exists");
        signal.mark_ready();

        assert!(
            matches!(signal.current_state(), InitializationState::Ready),
            "state should transition to Ready"
        );

        wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .expect("promise resolves");
        assert!(
            signal.wait_promise().is_err(),
            "promise should be dropped after resolution"
        );
    }

    #[wasm_bindgen_test(async)]
    async fn ready_signal_rejects_when_marked_failed() {
        let signal = ReadySignal::new();
        let promise = signal.wait_promise().expect("promise exists");
        signal.mark_failed("boom".into());

        match signal.current_state() {
            InitializationState::Failed(reason) => assert_eq!(reason, "boom"),
            other => panic!("expected Failed state, got {other:?}"),
        }

        let err = wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .expect_err("promise should reject");
        assert_eq!(err.as_string().as_deref(), Some("boom"));
    }

    #[wasm_bindgen_test(async)]
    async fn ready_signal_reset_returns_to_pending() {
        let signal = ReadySignal::new();
        signal.mark_ready();
        assert!(matches!(signal.current_state(), InitializationState::Ready));

        signal.reset();

        assert!(
            matches!(signal.current_state(), InitializationState::Pending),
            "state should be Pending after reset"
        );
        assert!(
            signal.wait_promise().is_ok(),
            "promise should exist after reset"
        );

        let promise = signal.wait_promise().expect("promise exists after reset");
        signal.mark_ready();
        wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .expect("promise should resolve after mark_ready");
    }

    #[wasm_bindgen_test(async)]
    async fn ready_signal_reset_from_failed_state() {
        let signal = ReadySignal::new();
        signal.mark_failed("initial failure".into());

        signal.reset();

        assert!(
            matches!(signal.current_state(), InitializationState::Pending),
            "state should be Pending after reset from failed"
        );

        signal.mark_ready();
        assert!(matches!(signal.current_state(), InitializationState::Ready));
    }
}
