use js_sys::Reflect;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{FileSystemDirectoryHandle, FileSystemGetDirectoryOptions, FileSystemRemoveOptions};

use crate::errors::SQLiteWasmDatabaseError;
use crate::utils::describe_js_value;

const SAHPOOL_DIR_NAME: &str = ".opfs-sahpool";

pub async fn delete_opfs_sahpool_directory() -> Result<(), SQLiteWasmDatabaseError> {
    let root = get_opfs_root().await?;

    let sahpool_dir = match get_directory_if_exists(&root, SAHPOOL_DIR_NAME).await? {
        Some(dir) => dir,
        None => return Ok(()),
    };

    delete_directory_contents(&sahpool_dir).await?;

    let remove_options = FileSystemRemoveOptions::new();
    remove_options.set_recursive(true);
    JsFuture::from(root.remove_entry_with_options(SAHPOOL_DIR_NAME, &remove_options))
        .await
        .map_err(|e| {
            SQLiteWasmDatabaseError::OpfsDeletionFailed(format!(
                "failed to remove sahpool directory: {}",
                describe_js_value(&e)
            ))
        })?;

    Ok(())
}

async fn get_opfs_root() -> Result<FileSystemDirectoryHandle, SQLiteWasmDatabaseError> {
    let navigator = web_sys::window()
        .map(|w| w.navigator())
        .or_else(|| {
            let global = js_sys::global();
            Reflect::get(&global, &JsValue::from_str("navigator"))
                .ok()
                .and_then(|n| n.dyn_into::<web_sys::Navigator>().ok())
        })
        .ok_or_else(|| {
            SQLiteWasmDatabaseError::OpfsDeletionFailed("navigator not available".into())
        })?;

    let storage = navigator.storage();
    JsFuture::from(storage.get_directory())
        .await
        .and_then(|v| v.dyn_into())
        .map_err(|e| {
            SQLiteWasmDatabaseError::OpfsDeletionFailed(format!(
                "failed to get OPFS root: {}",
                describe_js_value(&e)
            ))
        })
}

async fn get_directory_if_exists(
    parent: &FileSystemDirectoryHandle,
    name: &str,
) -> Result<Option<FileSystemDirectoryHandle>, SQLiteWasmDatabaseError> {
    let options = FileSystemGetDirectoryOptions::new();
    options.set_create(false);

    match JsFuture::from(parent.get_directory_handle_with_options(name, &options)).await {
        Ok(handle) => handle.dyn_into().map(Some).map_err(|e| {
            SQLiteWasmDatabaseError::OpfsDeletionFailed(format!(
                "directory handle type mismatch: {}",
                describe_js_value(&e)
            ))
        }),
        Err(e) => {
            if let Some(dom_ex) = e.dyn_ref::<web_sys::DomException>() {
                if dom_ex.name() == "NotFoundError" {
                    return Ok(None);
                }
            }
            Err(SQLiteWasmDatabaseError::OpfsDeletionFailed(format!(
                "failed to get directory '{}': {}",
                name,
                describe_js_value(&e)
            )))
        }
    }
}

async fn delete_directory_contents(
    dir: &FileSystemDirectoryHandle,
) -> Result<(), SQLiteWasmDatabaseError> {
    let entry_names = collect_entry_names(dir).await?;

    for name in entry_names {
        let remove_options = FileSystemRemoveOptions::new();
        remove_options.set_recursive(true);
        let _ = JsFuture::from(dir.remove_entry_with_options(&name, &remove_options)).await;
    }

    Ok(())
}

async fn collect_entry_names(
    dir: &FileSystemDirectoryHandle,
) -> Result<Vec<String>, SQLiteWasmDatabaseError> {
    let entries_iter = dir.entries();
    let mut names = Vec::new();

    loop {
        let next_fn = Reflect::get(&entries_iter, &JsValue::from_str("next"))
            .ok()
            .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
            .ok_or_else(|| {
                SQLiteWasmDatabaseError::OpfsDeletionFailed(
                    "entries iterator missing next method".into(),
                )
            })?;

        let next_promise = next_fn.call0(&entries_iter).map_err(|e| {
            SQLiteWasmDatabaseError::OpfsDeletionFailed(format!(
                "failed to call next(): {}",
                describe_js_value(&e)
            ))
        })?;

        let result = JsFuture::from(js_sys::Promise::from(next_promise))
            .await
            .map_err(|e| {
                SQLiteWasmDatabaseError::OpfsDeletionFailed(format!(
                    "iterator next() failed: {}",
                    describe_js_value(&e)
                ))
            })?;

        let done = Reflect::get(&result, &JsValue::from_str("done"))
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        if done {
            break;
        }

        if let Some(name) = Reflect::get(&result, &JsValue::from_str("value"))
            .ok()
            .and_then(|v| Reflect::get(&v, &JsValue::from_f64(0.0)).ok())
            .and_then(|v| v.as_string())
        {
            names.push(name);
        }
    }

    Ok(names)
}
