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
        {
            let mut state = self.state.borrow_mut();
            if matches!(*state, InitializationState::Ready) {
                return;
            }
            *state = InitializationState::Ready;
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
