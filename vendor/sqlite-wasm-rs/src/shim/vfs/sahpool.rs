//! opfs-sahpool vfs implementation, ported from sqlite-wasm
//!
//! <https://github.com/sqlite/sqlite/blob/master/ext/wasm/api/sqlite3-vfs-opfs-sahpool.c-pp.js>

use crate::{export::*, locker::RwLock};

use crate::fragile::FragileComfirmed;
use crate::locker::Mutex;
use js_sys::{
    Array, DataView, IteratorNext, Map, Math, Number, Object, Reflect, Set, Uint32Array, Uint8Array,
};
use once_cell::sync::Lazy;
use std::ffi::CString;
use std::sync::Arc;
use std::{collections::HashMap, ffi::CStr};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetDirectoryOptions,
    FileSystemGetFileOptions, FileSystemReadWriteOptions, FileSystemSyncAccessHandle, Url,
    WorkerGlobalScope,
};

const SECTOR_SIZE: usize = 4096;
const HEADER_MAX_PATH_SIZE: usize = 512;
const HEADER_FLAGS_SIZE: usize = 4;
const HEADER_DIGEST_SIZE: usize = 8;
const HEADER_CORPUS_SIZE: usize = HEADER_MAX_PATH_SIZE + HEADER_FLAGS_SIZE;
const HEADER_OFFSET_FLAGS: usize = HEADER_MAX_PATH_SIZE;
const HEADER_OFFSET_DIGEST: usize = HEADER_CORPUS_SIZE;
const HEADER_OFFSET_DATA: usize = SECTOR_SIZE;
const SQLITE_HEADER_SIZE: usize = 18;
const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

const PERSISTENT_FILE_TYPES: i32 =
    SQLITE_OPEN_MAIN_DB | SQLITE_OPEN_MAIN_JOURNAL | SQLITE_OPEN_SUPER_JOURNAL | SQLITE_OPEN_WAL;

fn combined_error(primary: OpfsSAHError, cleanup: OpfsSAHError) -> OpfsSAHError {
    OpfsSAHError::Custom(format!(
        "primary operation failed: {primary:?}; cleanup also failed, resource quarantined: {cleanup:?}"
    ))
}

fn validate_path_input(name: &str) -> Result<(), OpfsSAHError> {
    if name.is_empty() {
        return Err(OpfsSAHError::Custom("path must not be empty".into()));
    }
    if name.as_bytes().contains(&0) {
        return Err(OpfsSAHError::Custom(
            "path must not contain NUL bytes".into(),
        ));
    }
    Ok(())
}

fn canonicalize_path(name: &str) -> Result<String, OpfsSAHError> {
    validate_path_input(name)?;
    if name.contains("://") {
        return Err(OpfsSAHError::Custom(
            "URL schemes are not valid database paths".into(),
        ));
    }

    let mut segments = Vec::new();
    for segment in name.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            segment => segments.push(segment),
        }
    }
    let path = format!("/{}", segments.join("/"));
    Ok(path)
}

fn parent_url_path(name: &str) -> Result<String, OpfsSAHError> {
    validate_path_input(name)?;
    Url::new_with_base(name, "file://localhost/")
        .map(|url| url.pathname())
        .map_err(OpfsSAHError::GetPath)
}

fn new_path(name: &str) -> Result<String, OpfsSAHError> {
    let path = canonicalize_path(name)?;
    if path.len() >= HEADER_MAX_PATH_SIZE {
        return Err(OpfsSAHError::Custom(format!("Path too long: {path}")));
    }
    Ok(path)
}

fn new_utility_path(name: &str) -> Result<String, OpfsSAHError> {
    validate_path_input(name)?;
    let bytes = name.as_bytes();
    if matches!(bytes.first(), Some(byte) if *byte <= b' ')
        || matches!(bytes.last(), Some(byte) if *byte <= b' ')
        || bytes
            .iter()
            .any(|byte| matches!(byte, b'\t' | b'\r' | b'\n'))
    {
        return Err(OpfsSAHError::Custom(format!(
            "WHATWG-trimmed whitespace and control bytes are not valid utility destinations: {name:?}"
        )));
    }
    let path = new_path(name)?;
    if parent_url_path(name)? != parent_url_path(&path)? {
        return Err(OpfsSAHError::Custom(format!(
            "Utility path identity would change after publication; use an explicit ordinary filesystem path: {name}"
        )));
    }
    Ok(path)
}

fn path_identities(name: &str) -> Result<Vec<String>, OpfsSAHError> {
    validate_path_input(name)?;
    let mut paths = vec![name.to_owned()];
    if let Ok(path) = canonicalize_path(name) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    let legacy = parent_url_path(name)?;
    if !paths.contains(&legacy) {
        paths.push(legacy.clone());
    }
    if let Ok(canonical_legacy) = canonicalize_path(&legacy) {
        if !paths.contains(&canonical_legacy) {
            paths.push(canonical_legacy);
        }
    }
    Ok(paths)
}

fn paths_overlap(left: &str, right: &str) -> Result<bool, OpfsSAHError> {
    let left = path_identities(left)?;
    let right = path_identities(right)?;
    Ok(left.iter().any(|path| right.contains(path)))
}

// sqlite-wasm-rs 0.3.0 wrote a zero digest. Reuse a VFS-irrelevant flag to
// distinguish new metadata while retaining compatibility with existing pools.
const FLAG_COMPUTE_DIGEST_V2: i32 = SQLITE_OPEN_MEMORY;

static VFS2SAH: Lazy<RwLock<HashMap<usize, Arc<OpfsSAH>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

fn pool(vfs: *mut sqlite3_vfs) -> Arc<FragileComfirmed<OpfsSAHPool>> {
    VFS2SAH.read().get(&(vfs as usize)).unwrap().pool.clone()
}

fn read_write_options(at: f64) -> FileSystemReadWriteOptions {
    let options = FileSystemReadWriteOptions::new();
    options.set_at(at);
    options
}

unsafe fn file2vfs(file: *mut sqlite3_file) -> *mut sqlite3_vfs {
    (*(file.cast::<OpfsFile>())).vfs
}

fn compute_digest(byte_array: &Uint8Array, file_flags: u32) -> Uint32Array {
    let digest = Uint32Array::new_with_length(2);
    if file_flags & FLAG_COMPUTE_DIGEST_V2 as u32 == 0 {
        return digest;
    }

    let mut h1 = 0xdead_beefu32;
    let mut h2 = 0x41c6_ce57u32;
    for index in 0..byte_array.length() {
        let value = byte_array.get_index(index) as u32;
        h1 = (h1 ^ value).wrapping_mul(2_654_435_761);
        h2 = (h2 ^ value).wrapping_mul(104_729);
    }
    digest.set_index(0, h1);
    digest.set_index(1, h2);
    digest
}

fn get_random_name() -> String {
    let random = Number::from(Math::random())
        .to_string_with_radix(36)
        .unwrap();
    random.slice(2, random.length()).as_string().unwrap()
}

#[repr(C)]
struct OpfsFile {
    io_methods: sqlite3_file,
    vfs: *mut sqlite3_vfs,
}

struct FileObject {
    path: String,
    flags: i32,
    sah: FileSystemSyncAccessHandle,
}

impl FileObject {
    fn new(obj: Object) -> Result<Self, OpfsSAHError> {
        let path = Reflect::get(&obj, &JsValue::from("path"))
            .map_err(OpfsSAHError::Reflect)?
            .as_string()
            .ok_or_else(|| OpfsSAHError::Custom("path not string".into()))?;

        let flags = Reflect::get(&obj, &JsValue::from("flags"))
            .map_err(OpfsSAHError::Reflect)?
            .as_f64()
            .ok_or_else(|| OpfsSAHError::Custom("flags not number".into()))?
            as i32;

        let sah = Reflect::get(&obj, &JsValue::from("sah"))
            .map_err(OpfsSAHError::Reflect)?
            .into();

        Ok(Self { path, flags, sah })
    }
}

/// Class for managing OPFS-related state for the OPFS
/// SharedAccessHandle Pool sqlite3_vfs.
struct OpfsSAHPool {
    /// Directory handle to the subdir of vfs root which holds
    /// the randomly-named "opaque" files. This subdir exists in the
    /// hope that we can eventually support client-created files in
    dh_opaque: FileSystemDirectoryHandle,
    /// Buffer used by [sg]etAssociatedPath()
    ap_body: Uint8Array,
    /// DataView for self.apBody
    dv_body: DataView,
    /// Maps client-side file names to SAHs
    map_filename_to_sah: Map,
    /// Logical paths claimed by imports which are not published yet.
    reserved_paths: Set,
    /// Set of currently-unused SAHs
    available_sah: Set,
    /// Maps SAHs to their opaque file names
    map_sah_to_name: Map,
    /// Maps (sqlite3_file*) to xOpen's file objects
    map_s3_file_to_o_file: Map,
    /// Store last_error
    ///
    /// Never poison, unwrap `lock()` is fine
    last_error: Mutex<Option<(i32, String)>>,
    #[cfg(test)]
    association_failures_after_body: Mutex<u32>,
}

impl OpfsSAHPool {
    async fn new(options: &OpfsSAHPoolCfg) -> Result<OpfsSAHPool, OpfsSAHError> {
        const OPAQUE_DIR_NAME: &str = ".opaque";

        let vfs_dir = &options.directory;
        let capacity = options.initial_capacity;
        let clear_files = options.clear_on_init;

        let create_option = FileSystemGetDirectoryOptions::new();
        create_option.set_create(true);

        let mut handle: FileSystemDirectoryHandle = JsFuture::from(
            js_sys::global()
                .dyn_into::<WorkerGlobalScope>()
                .map_err(|_| OpfsSAHError::NotSuported)?
                .navigator()
                .storage()
                .get_directory(),
        )
        .await
        .map_err(OpfsSAHError::GetDirHandle)?
        .into();

        for dir in vfs_dir.split('/').filter(|x| !x.is_empty()) {
            let next =
                JsFuture::from(handle.get_directory_handle_with_options(dir, &create_option))
                    .await
                    .map_err(OpfsSAHError::GetDirHandle)?
                    .into();
            handle = next;
        }

        let dh_opaque = JsFuture::from(
            handle.get_directory_handle_with_options(OPAQUE_DIR_NAME, &create_option),
        )
        .await
        .map_err(OpfsSAHError::GetDirHandle)?
        .into();

        let ap_body = Uint8Array::new_with_length(HEADER_CORPUS_SIZE as _);
        let dv_body = DataView::new(
            &ap_body.buffer(),
            ap_body.byte_offset() as usize,
            (ap_body.byte_length() - ap_body.byte_offset()) as usize,
        );

        let pool = Self {
            dh_opaque,
            ap_body,
            dv_body,
            map_filename_to_sah: Map::new(),
            reserved_paths: Set::default(),
            available_sah: Set::default(),
            map_sah_to_name: Map::new(),
            map_s3_file_to_o_file: Map::new(),
            last_error: Mutex::new(None),
            #[cfg(test)]
            association_failures_after_body: Mutex::new(0),
        };
        pool.acquire_access_handles(clear_files).await?;
        if pool.get_capacity() == 0 {
            pool.add_capacity(capacity).await?;
        }

        Ok(pool)
    }

    /// Adds n files to the pool's capacity. This change is
    /// persistent across settings. Returns a Promise which resolves
    /// to the new capacity.
    async fn add_capacity(&self, n: u32) -> Result<u32, OpfsSAHError> {
        for _ in 0..n {
            let name = get_random_name();
            let handle: FileSystemFileHandle =
                JsFuture::from(self.dh_opaque.get_file_handle_with_options(&name, &{
                    let options = FileSystemGetFileOptions::new();
                    options.set_create(true);
                    options
                }))
                .await
                .map_err(OpfsSAHError::GetFileHandle)?
                .into();
            let sah: FileSystemSyncAccessHandle =
                JsFuture::from(handle.create_sync_access_handle())
                    .await
                    .map_err(OpfsSAHError::CreateSyncAccessHandle)?
                    .into();
            self.map_sah_to_name.set(&sah, &JsValue::from(name));
            self.set_associated_path(&sah, "", 0)?;
        }
        Ok(self.get_capacity())
    }

    /// Reduce capacity by n, but can only reduce up to the limit
    /// of currently-available SAHs. Returns a Promise which resolves
    /// to the number of slots really removed.
    async fn reduce_capacity(&self, n: u32) -> Result<u32, OpfsSAHError> {
        let mut result = 0;
        for sah in Array::from(&self.available_sah) {
            if result == n || self.get_capacity() == self.get_file_count() {
                break;
            }
            let sah = FileSystemSyncAccessHandle::from(sah);

            let name = self.map_sah_to_name.get(&sah);
            assert!(!name.is_undefined(), "name must exists");
            let name = name.as_string().unwrap();

            sah.close();
            JsFuture::from(self.dh_opaque.remove_entry(&name))
                .await
                .map_err(OpfsSAHError::RemoveEntity)?;
            self.map_sah_to_name.delete(&sah);
            self.available_sah.delete(&sah);
            result += 1;
        }
        Ok(result)
    }

    /// Current pool capacity.
    fn get_capacity(&self) -> u32 {
        self.map_sah_to_name.size()
    }

    /// Current number of in-use files from pool.
    fn get_file_count(&self) -> u32 {
        self.map_filename_to_sah.size()
    }

    /// Returns an array of the names of all
    /// currently-opened client-specified filenames.
    fn get_file_names(&self) -> Vec<String> {
        let mut result = vec![];
        for name in self.map_filename_to_sah.keys().into_iter().flatten() {
            result.push(name.as_string().unwrap());
        }
        result
    }

    /// Given an SAH, returns the client-specified name of
    /// that file by extracting it from the SAH's header.
    /// On error, it disassociates SAH from the pool and
    /// returns an empty string.
    fn get_associated_path(
        &self,
        sah: &FileSystemSyncAccessHandle,
    ) -> Result<Option<String>, OpfsSAHError> {
        let body_read = sah
            .read_with_buffer_source_and_options(&self.ap_body, &read_write_options(0.0))
            .map_err(OpfsSAHError::Read)?;
        if body_read != HEADER_CORPUS_SIZE as f64 {
            return Err(OpfsSAHError::Custom(format!(
                "Expected to read {HEADER_CORPUS_SIZE} metadata bytes but read {body_read}."
            )));
        }
        let flags = self.dv_body.get_uint32(HEADER_OFFSET_FLAGS);

        // size is 2
        let file_digest = Uint32Array::new_with_length(HEADER_DIGEST_SIZE as u32 / 4);
        let digest_read = sah
            .read_with_buffer_source_and_options(
                &file_digest,
                &read_write_options(HEADER_OFFSET_DIGEST as f64),
            )
            .map_err(OpfsSAHError::Read)?;
        if digest_read != HEADER_DIGEST_SIZE as f64 {
            return Err(OpfsSAHError::Custom(format!(
                "Expected to read {HEADER_DIGEST_SIZE} digest bytes but read {digest_read}."
            )));
        }

        let comp_digest = compute_digest(&self.ap_body, flags);
        if !Array::from(&file_digest)
            .every(&mut |v, i, _| v.as_f64().unwrap() as u32 == comp_digest.get_index(i))
        {
            return Err(OpfsSAHError::Custom(
                "SAH metadata digest mismatch; refusing to modify the stored database".into(),
            ));
        }

        if self.ap_body.get_index(0) != 0
            && ((flags & SQLITE_OPEN_DELETEONCLOSE as u32 != 0)
                || (flags & PERSISTENT_FILE_TYPES as u32) == 0)
        {
            self.set_associated_path(sah, "", 0)?;
            return Ok(None);
        }

        let nul_index =
            Array::from(&self.ap_body).find_index(&mut |x, _, _| x.as_f64().unwrap() as u8 == 0);
        let path_size = if nul_index < 0 {
            HEADER_MAX_PATH_SIZE as u32
        } else {
            nul_index as u32
        };
        if path_size == 0 {
            sah.truncate_with_u32(HEADER_OFFSET_DATA as u32)
                .map_err(OpfsSAHError::Truncate)?;
            return Ok(None);
        }
        let path_bytes = self.ap_body.subarray(0, path_size);
        let mut path = vec![0; path_size as usize];
        for idx in 0..path_size {
            // why not `copy_to`?
            //
            // see <https://github.com/rustwasm/wasm-bindgen/issues/4395>
            path[idx as usize] = path_bytes.get_index(idx);
        }
        // set_associated_path ensures that it is utf8
        let path = String::from_utf8(path).unwrap();
        Ok(Some(path))
    }

    /// Stores the given client-defined path and SQLITE_OPEN_xyz flags
    /// into the given SAH. If path is an empty string then the file is
    /// disassociated from the pool but its previous name is preserved
    /// in the metadata.
    fn set_associated_path(
        &self,
        sah: &FileSystemSyncAccessHandle,
        path: &str,
        flags: i32,
    ) -> Result<(), OpfsSAHError> {
        self.set_associated_path_with_limit(sah, path, flags, false)
    }

    fn restore_legacy_associated_path(
        &self,
        sah: &FileSystemSyncAccessHandle,
        path: &str,
        flags: i32,
    ) -> Result<(), OpfsSAHError> {
        self.set_associated_path_with_limit(sah, path, flags, true)
    }

    fn set_associated_path_with_limit(
        &self,
        sah: &FileSystemSyncAccessHandle,
        path: &str,
        flags: i32,
        allow_legacy_max: bool,
    ) -> Result<(), OpfsSAHError> {
        if (allow_legacy_max && HEADER_MAX_PATH_SIZE < path.len())
            || (!allow_legacy_max && HEADER_MAX_PATH_SIZE <= path.len())
        {
            return Err(OpfsSAHError::Custom(format!("Path too long: {path}")));
        }
        for (idx, byte) in path.bytes().enumerate() {
            // why not `copy_from`?
            //
            // see <https://github.com/rustwasm/wasm-bindgen/issues/4395>
            self.ap_body.set_index(idx as u32, byte);
        }

        let flags = if !path.is_empty() && flags != 0 {
            flags | FLAG_COMPUTE_DIGEST_V2
        } else {
            flags
        };

        self.ap_body
            .fill(0, path.len() as u32, HEADER_MAX_PATH_SIZE as u32);
        self.dv_body.set_uint32(HEADER_OFFSET_FLAGS, flags as u32);

        let digest = compute_digest(&self.ap_body, flags as u32);

        let body_written = sah
            .write_with_js_u8_array_and_options(&self.ap_body, &read_write_options(0.0))
            .map_err(OpfsSAHError::Write)?;
        if body_written != self.ap_body.byte_length() as f64 {
            return Err(OpfsSAHError::Custom(format!(
                "Expected to write {} metadata bytes but wrote {}.",
                self.ap_body.byte_length(),
                body_written
            )));
        }
        #[cfg(test)]
        {
            let mut remaining = self.association_failures_after_body.lock();
            if *remaining > 0 {
                *remaining -= 1;
                return Err(OpfsSAHError::Custom(
                    "injected metadata failure after body write".into(),
                ));
            }
        }
        let digest_written = sah
            .write_with_buffer_source_and_options(
                &digest,
                &read_write_options(HEADER_OFFSET_DIGEST as f64),
            )
            .map_err(OpfsSAHError::Write)?;
        if digest_written != digest.byte_length() as f64 {
            return Err(OpfsSAHError::Custom(format!(
                "Expected to write {} digest bytes but wrote {}.",
                digest.byte_length(),
                digest_written
            )));
        }
        sah.flush().map_err(OpfsSAHError::Flush)?;

        if path.is_empty() {
            sah.truncate_with_u32(HEADER_OFFSET_DATA as u32)
                .map_err(OpfsSAHError::Truncate)?;
            self.available_sah.add(sah);
        } else {
            self.map_filename_to_sah.set(&JsValue::from(path), sah);
            self.available_sah.delete(sah);
        }

        Ok(())
    }

    /// Opens all files under self.dh_opaque and acquires
    /// a SAH for each. returns a Promise which resolves to no value
    /// but completes once all SAHs are acquired. If acquiring an SAH
    /// throws, SAHPool.$error will contain the corresponding
    /// exception.
    ///
    /// If clearFiles is true, the client-stored state of each file is
    /// cleared when its handle is acquired, including its name, flags,
    /// and any data stored after the metadata block.
    async fn acquire_access_handles(&self, clear_files: bool) -> Result<(), OpfsSAHError> {
        let mut files = vec![];
        let iter = self.dh_opaque.entries();
        while let Ok(future) = iter.next() {
            let next: IteratorNext = JsFuture::from(future)
                .await
                .map_err(OpfsSAHError::IterHandle)?
                .into();
            if next.done() {
                break;
            }
            let array: Array = next.value().into();
            let key = array.get(0);
            let value = array.get(1);
            let kind = Reflect::get(&value, &JsValue::from("kind"))
                .map_err(OpfsSAHError::Reflect)?
                .as_string();
            if kind.as_deref() == Some("file") {
                files.push((key, FileSystemFileHandle::from(value)));
            }
        }

        let fut = async {
            for (file, handle) in files {
                let sah = JsFuture::from(handle.create_sync_access_handle())
                    .await
                    .map_err(OpfsSAHError::CreateSyncAccessHandle)?;
                self.map_sah_to_name.set(&sah, &file);
                let sah = FileSystemSyncAccessHandle::from(sah);
                if clear_files {
                    sah.truncate_with_u32(HEADER_OFFSET_DATA as u32)
                        .map_err(OpfsSAHError::Truncate)?;
                    self.set_associated_path(&sah, "", 0)?;
                } else if let Some(path) = self.get_associated_path(&sah)? {
                    self.map_filename_to_sah.set(&JsValue::from(path), &sah);
                } else {
                    self.available_sah.add(&sah);
                }
            }
            Ok::<_, OpfsSAHError>(())
        };

        if let Err(e) = fut.await {
            self.store_err(&e, None);
            self.release_access_handles();
            return Err(e);
        }

        Ok(())
    }

    /// Releases all currently-opened SAHs. The only legal
    /// operation after this is acquireAccessHandles().
    fn release_access_handles(&self) {
        for sah in self.map_sah_to_name.keys().into_iter().flatten() {
            let sah = FileSystemSyncAccessHandle::from(sah);
            sah.close();
        }
        self.map_sah_to_name.clear();
        self.map_filename_to_sah.clear();
        self.available_sah.clear();
    }

    /// Pops this object's Error object and returns
    /// it (a falsy value if no error is set).
    fn pop_err(&self) -> Option<(i32, String)> {
        self.last_error.lock().take()
    }

    /// Sets e (an Error object) as this object's current error. Pass a
    /// falsy (or no) value to clear it. If code is truthy it is
    /// assumed to be an SQLITE_xxx result code, defaulting to
    /// SQLITE_IOERR if code is falsy.
    fn store_err(&self, err: &OpfsSAHError, code: Option<i32>) -> i32 {
        let code = code.unwrap_or(SQLITE_IOERR);
        self.last_error.lock().replace((code, format!("{:?}", err)));
        code
    }

    /// Given an (sqlite3_file*), returns the mapped
    /// xOpen file object.
    fn get_o_file_for_s3_file(
        &self,
        p_file: *mut sqlite3_file,
    ) -> Result<FileObject, OpfsSAHError> {
        let file = self.map_s3_file_to_o_file.get(&JsValue::from(p_file));
        if file.is_undefined() {
            return Err(OpfsSAHError::Custom("open file not exists".into()));
        }
        FileObject::new(file.into())
    }

    /// Maps or unmaps (if file is falsy) the given (sqlite3_file*)
    /// to an xOpen file object and to this pool object.
    fn map_s3_file_to_o_file(&self, p_file: *mut sqlite3_file, file: Option<Object>) {
        if let Some(file) = file {
            self.map_s3_file_to_o_file
                .set(&JsValue::from(p_file), &JsValue::from(file));
        } else {
            self.map_s3_file_to_o_file.delete(&JsValue::from(p_file));
        }
    }

    fn is_path_open(&self, path: &str) -> Result<bool, OpfsSAHError> {
        for file in self.map_s3_file_to_o_file.values().into_iter().flatten() {
            if paths_overlap(path, &FileObject::new(file.into())?.path)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn unique_temp_path(&self) -> Result<String, OpfsSAHError> {
        for _ in 0..100 {
            let path = new_path(&format!("/.sqlite-temp-{}", get_random_name()))?;
            if !self.is_path_claimed(&path)? {
                return Ok(path);
            }
        }
        Err(OpfsSAHError::Custom(
            "Could not allocate a unique temporary database path.".into(),
        ))
    }

    /// Removes the association of the given client-specified file
    /// name (JS string) from the pool. Returns true if a mapping
    /// is found, else false.
    fn delete_path(&self, path: &str) -> Result<bool, OpfsSAHError> {
        self.delete_path_with_mode(path, false)
    }

    fn delete_sqlite_path(&self, path: &str) -> Result<bool, OpfsSAHError> {
        self.delete_path_with_mode(path, true)
    }

    fn delete_path_with_mode(&self, path: &str, sqlite: bool) -> Result<bool, OpfsSAHError> {
        if self.is_path_reservation_claimed(path)? {
            return Err(OpfsSAHError::Custom(format!(
                "Path is reserved and cannot be unlinked: {path}"
            )));
        }
        let resolved = if sqlite {
            self.resolve_sqlite_path(path)?
        } else {
            self.resolve_utility_path(path)?
        };
        let Some((stored_path, sah)) = resolved else {
            return Ok(false);
        };
        if self.is_path_open(path)? {
            return Err(OpfsSAHError::Custom(format!(
                "Path is open and cannot be unlinked: {path}"
            )));
        }
        self.set_associated_path(&sah, "", 0)?;
        self.map_filename_to_sah.delete(&JsValue::from(stored_path));
        Ok(true)
    }

    /// Reads the original SQLite filename. Identity resolution deliberately
    /// happens later so parent WHATWG-URL behavior can use the raw spelling.
    fn get_path(&self, name: *const ::std::os::raw::c_char) -> Result<String, OpfsSAHError> {
        if name.is_null() {
            return Err(OpfsSAHError::Custom("name is null ptr".into()));
        }
        let name = unsafe {
            CStr::from_ptr(name)
                .to_str()
                .map_err(|e| OpfsSAHError::Custom(format!("{e:?}")))?
        };
        validate_path_input(name)?;
        Ok(name.to_owned())
    }

    fn resolve_candidates(
        &self,
        candidates: impl IntoIterator<Item = String>,
    ) -> Result<Option<(String, FileSystemSyncAccessHandle)>, OpfsSAHError> {
        for candidate in candidates {
            let sah = self.map_filename_to_sah.get(&JsValue::from(&candidate));
            if !sah.is_undefined() {
                return Ok(Some((candidate, FileSystemSyncAccessHandle::from(sah))));
            }
        }
        Ok(None)
    }

    fn resolve_utility_path(
        &self,
        raw: &str,
    ) -> Result<Option<(String, FileSystemSyncAccessHandle)>, OpfsSAHError> {
        validate_path_input(raw)?;
        let exact = self.map_filename_to_sah.get(&JsValue::from(raw));
        if !exact.is_undefined() {
            return Ok(Some((
                raw.to_owned(),
                FileSystemSyncAccessHandle::from(exact),
            )));
        }
        let mut candidates = Vec::new();
        if let Ok(literal) = canonicalize_path(raw) {
            candidates.push(literal);
        }
        let legacy = parent_url_path(raw)?;
        if !candidates.contains(&legacy) {
            candidates.push(legacy.clone());
        }
        if let Ok(canonical_legacy) = canonicalize_path(&legacy) {
            if !candidates.contains(&canonical_legacy) {
                candidates.push(canonical_legacy);
            }
        }
        self.resolve_candidates(candidates)
    }

    fn resolve_sqlite_path(
        &self,
        raw: &str,
    ) -> Result<Option<(String, FileSystemSyncAccessHandle)>, OpfsSAHError> {
        validate_path_input(raw)?;
        let legacy = parent_url_path(raw)?;
        let mut candidates = vec![legacy.clone()];
        if let Ok(canonical_legacy) = canonicalize_path(&legacy) {
            if !candidates.contains(&canonical_legacy) {
                candidates.push(canonical_legacy);
            }
        }
        if let Ok(literal) = canonicalize_path(raw) {
            if !candidates.contains(&literal) {
                candidates.push(literal);
            }
        }
        if !candidates.iter().any(|path| path == raw) {
            candidates.push(raw.to_owned());
        }
        self.resolve_candidates(candidates)
    }

    fn any_matching_key(&self, keys: &Set, path: &str) -> Result<bool, OpfsSAHError> {
        for key in keys.keys().into_iter().flatten() {
            if paths_overlap(path, &key.as_string().unwrap())? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn is_path_claimed(&self, path: &str) -> Result<bool, OpfsSAHError> {
        for key in self.map_filename_to_sah.keys().into_iter().flatten() {
            if paths_overlap(path, &key.as_string().unwrap())? {
                return Ok(true);
            }
        }
        self.any_matching_key(&self.reserved_paths, path)
    }

    fn is_path_reservation_claimed(&self, path: &str) -> Result<bool, OpfsSAHError> {
        self.any_matching_key(&self.reserved_paths, path)
    }

    fn reserve_path(&self, path: &str) -> Result<(), OpfsSAHError> {
        if self.is_path_claimed(path)? {
            return Err(OpfsSAHError::Custom(format!(
                "Destination path already exists or is reserved: {path}"
            )));
        }
        self.reserved_paths.add(&JsValue::from(path));
        Ok(())
    }

    fn release_path(&self, path: &str) {
        self.reserved_paths.delete(&JsValue::from(path));
    }

    fn quarantine_path(&self, path: &str) {
        self.reserved_paths.add(&JsValue::from(path));
    }

    /// Returns the next available SAH without removing
    /// it from the set.
    fn next_available_sah(&self) -> Option<FileSystemSyncAccessHandle> {
        self.available_sah
            .keys()
            .next()
            .ok()
            .filter(|x| !x.done())
            .map(|x| x.value().into())
    }

    fn export_file(&self, name: &str) -> Result<Vec<u8>, OpfsSAHError> {
        let Some((_, sah)) = self.resolve_utility_path(name)? else {
            return Err(OpfsSAHError::Custom("File not found:".into()));
        };
        let n = sah.get_size().map_err(OpfsSAHError::GetSize)? - HEADER_OFFSET_DATA as f64;
        let n = n.max(0.0) as usize;
        let mut data = vec![0; n];
        if n > 0 {
            let read = sah
                .read_with_u8_array_and_options(
                    &mut data,
                    &read_write_options(HEADER_OFFSET_DATA as f64),
                )
                .map_err(OpfsSAHError::Read)?;
            if read != n as f64 {
                return Err(OpfsSAHError::Custom(format!(
                    "Expected to read {} bytes but read {}.",
                    n, read
                )));
            }
        }
        Ok(data)
    }

    fn import_db(&self, path: &str, reservation: &str, bytes: &[u8]) -> Result<(), OpfsSAHError> {
        let page_size = sqlite_page_size(bytes)?;
        let length = u64::try_from(bytes.len())
            .map_err(|_| OpfsSAHError::Custom("SQLite database is too large to import.".into()))?;
        validate_database_length(length, page_size)?;
        checked_data_offset(length)?;

        self.reserve_path(reservation)?;
        let sah = match self.next_available_sah() {
            Some(sah) => sah,
            None => {
                self.release_path(reservation);
                return Err(OpfsSAHError::Custom(
                    "No available handles to import to.".into(),
                ));
            }
        };
        self.available_sah.delete(&sah);
        let result = (|| {
            sah.truncate_with_u32(HEADER_OFFSET_DATA as u32)
                .map_err(OpfsSAHError::Truncate)?;
            let write = sah
                .write_with_u8_array_and_options(
                    bytes,
                    &read_write_options(HEADER_OFFSET_DATA as f64),
                )
                .map_err(OpfsSAHError::Write)?;
            if write != length as f64 {
                return Err(OpfsSAHError::Custom(format!(
                    "Expected to write {} bytes but wrote {}.",
                    length, write
                )));
            }

            let journal_mode = [1, 1];
            let written = sah
                .write_with_u8_array_and_options(
                    &journal_mode,
                    &read_write_options((HEADER_OFFSET_DATA + 18) as f64),
                )
                .map_err(OpfsSAHError::Write)?;
            if written != journal_mode.len() as f64 {
                return Err(OpfsSAHError::Custom(format!(
                    "Expected to write {} journal-mode bytes but wrote {}.",
                    journal_mode.len(),
                    written
                )));
            }
            self.set_associated_path(&sah, path, SQLITE_OPEN_MAIN_DB)
        })();

        if let Err(primary) = result {
            return match self.set_associated_path(&sah, "", 0) {
                Ok(()) => {
                    self.release_path(reservation);
                    Err(primary)
                }
                Err(cleanup) => Err(combined_error(primary, cleanup)),
            };
        }
        self.release_path(reservation);
        Ok(())
    }
}

fn sqlite_page_size(header: &[u8]) -> Result<u64, OpfsSAHError> {
    if header.len() < SQLITE_HEADER_SIZE || &header[..16] != b"SQLite format 3\0" {
        return Err(OpfsSAHError::Custom(
            "Input does not contain a valid SQLite database header.".into(),
        ));
    }

    let encoded = u16::from_be_bytes([header[16], header[17]]);
    let page_size = if encoded == 1 {
        65_536
    } else {
        u64::from(encoded)
    };
    if page_size != 65_536 && (!(512..=32_768).contains(&page_size) || !page_size.is_power_of_two())
    {
        return Err(OpfsSAHError::Custom(format!(
            "Invalid SQLite database page size: {page_size}."
        )));
    }
    Ok(page_size)
}

fn validate_database_length(length: u64, page_size: u64) -> Result<(), OpfsSAHError> {
    if length < page_size || length % page_size != 0 {
        return Err(OpfsSAHError::Custom(format!(
            "SQLite database size {length} is not a positive multiple of its {page_size}-byte page size."
        )));
    }
    Ok(())
}

fn checked_data_offset(length: u64) -> Result<f64, OpfsSAHError> {
    let offset = (HEADER_OFFSET_DATA as u64)
        .checked_add(length)
        .ok_or_else(|| OpfsSAHError::Custom("SQLite database offset overflow.".into()))?;
    if offset > JS_MAX_SAFE_INTEGER {
        return Err(OpfsSAHError::Custom(
            "SQLite database offset exceeds JavaScript's safe integer range.".into(),
        ));
    }
    Ok(offset as f64)
}

unsafe extern "C" fn xCheckReservedLock(
    pFile: *mut sqlite3_file,
    pResOut: *mut ::std::os::raw::c_int,
) -> ::std::os::raw::c_int {
    let vfs = file2vfs(pFile);
    let pool = pool(vfs);
    pool.pop_err();

    *pResOut = 1;
    SQLITE_OK
}

unsafe extern "C" fn xClose(pFile: *mut sqlite3_file) -> ::std::os::raw::c_int {
    let vfs = file2vfs(pFile);
    let pool = pool(vfs);
    pool.pop_err();

    let f = || {
        if let Ok(file) = pool.get_o_file_for_s3_file(pFile) {
            pool.map_s3_file_to_o_file(pFile, None);
            file.sah.flush().map_err(OpfsSAHError::Flush)?;
            if (file.flags & SQLITE_OPEN_DELETEONCLOSE) != 0 {
                pool.delete_path(&file.path)?;
            }
        }
        Ok::<_, OpfsSAHError>(())
    };

    if let Err(e) = f() {
        return pool.store_err(&e, Some(SQLITE_IOERR));
    }
    SQLITE_OK
}

unsafe extern "C" fn xDeviceCharacteristics(_pFile: *mut sqlite3_file) -> ::std::os::raw::c_int {
    SQLITE_IOCAP_UNDELETABLE_WHEN_OPEN
}

unsafe extern "C" fn xFileControl(
    _pFile: *mut sqlite3_file,
    _op: ::std::os::raw::c_int,
    _pArg: *mut ::std::os::raw::c_void,
) -> ::std::os::raw::c_int {
    SQLITE_NOTFOUND
}

unsafe extern "C" fn xFileSize(
    pFile: *mut sqlite3_file,
    pSize: *mut sqlite3_int64,
) -> ::std::os::raw::c_int {
    let vfs = file2vfs(pFile);
    let pool = pool(vfs);
    pool.pop_err();

    let result = pool.get_o_file_for_s3_file(pFile).and_then(|file| {
        let size = file.sah.get_size().map_err(OpfsSAHError::GetSize)?;
        if size < HEADER_OFFSET_DATA as f64 {
            return Err(OpfsSAHError::Custom(
                "OPFS file is smaller than its metadata header".into(),
            ));
        }
        *pSize = size as i64 - HEADER_OFFSET_DATA as i64;
        Ok(())
    });
    if let Err(error) = result {
        return pool.store_err(&error, Some(SQLITE_IOERR_FSTAT));
    }
    SQLITE_OK
}

unsafe extern "C" fn xLock(
    _pFile: *mut sqlite3_file,
    _eLock: ::std::os::raw::c_int,
) -> ::std::os::raw::c_int {
    SQLITE_OK
}

unsafe extern "C" fn xRead(
    pFile: *mut sqlite3_file,
    zBuf: *mut ::std::os::raw::c_void,
    iAmt: ::std::os::raw::c_int,
    iOfst: sqlite3_int64,
) -> ::std::os::raw::c_int {
    let vfs = file2vfs(pFile);
    let pool = pool(vfs);
    pool.pop_err();

    let f = || {
        let file = pool.get_o_file_for_s3_file(pFile)?;
        let slice = std::slice::from_raw_parts_mut(zBuf.cast::<u8>(), iAmt as usize);

        let n_read = file
            .sah
            .read_with_u8_array_and_options(
                slice,
                &read_write_options((HEADER_OFFSET_DATA as i64 + iOfst) as f64),
            )
            .map_err(OpfsSAHError::Read)?;

        if (n_read as i32) < iAmt {
            slice[n_read as usize..iAmt as usize].fill(0);
            return Ok(SQLITE_IOERR_SHORT_READ);
        }

        Ok::<i32, OpfsSAHError>(SQLITE_OK)
    };

    match f() {
        Ok(ret) => ret,
        Err(e) => pool.store_err(&e, Some(SQLITE_IOERR)),
    }
}

unsafe extern "C" fn xSectorSize(_pFile: *mut sqlite3_file) -> ::std::os::raw::c_int {
    SECTOR_SIZE as i32
}

unsafe extern "C" fn xSync(
    pFile: *mut sqlite3_file,
    _flags: ::std::os::raw::c_int,
) -> ::std::os::raw::c_int {
    let vfs = file2vfs(pFile);
    let pool = pool(vfs);
    pool.pop_err();

    if let Err(e) = pool
        .get_o_file_for_s3_file(pFile)
        .and_then(|file| file.sah.flush().map_err(OpfsSAHError::Flush))
    {
        return pool.store_err(&e, Some(SQLITE_IOERR));
    }

    SQLITE_OK
}

unsafe extern "C" fn xTruncate(
    pFile: *mut sqlite3_file,
    size: sqlite3_int64,
) -> ::std::os::raw::c_int {
    let vfs = file2vfs(pFile);
    let pool = pool(vfs);
    pool.pop_err();

    if let Err(e) = pool.get_o_file_for_s3_file(pFile).and_then(|file| {
        file.sah
            .truncate_with_f64((HEADER_OFFSET_DATA as i64 + size) as f64)
            .map_err(OpfsSAHError::Truncate)
    }) {
        return pool.store_err(&e, Some(SQLITE_IOERR));
    }

    SQLITE_OK
}

unsafe extern "C" fn xUnlock(
    _pFile: *mut sqlite3_file,
    _eLock: ::std::os::raw::c_int,
) -> ::std::os::raw::c_int {
    SQLITE_OK
}

unsafe extern "C" fn xWrite(
    pFile: *mut sqlite3_file,
    zBuf: *const ::std::os::raw::c_void,
    iAmt: ::std::os::raw::c_int,
    iOfst: sqlite3_int64,
) -> ::std::os::raw::c_int {
    let vfs = file2vfs(pFile);
    let pool = pool(vfs);
    pool.pop_err();

    let f = || {
        let file = pool.get_o_file_for_s3_file(pFile)?;
        let slice = std::slice::from_raw_parts(zBuf.cast::<u8>(), iAmt as usize);

        let n_write = file
            .sah
            .write_with_u8_array_and_options(
                slice,
                &read_write_options((HEADER_OFFSET_DATA as i64 + iOfst) as f64),
            )
            .map_err(OpfsSAHError::Read)?;

        let ret = if iAmt == n_write as i32 {
            SQLITE_OK
        } else {
            SQLITE_ERROR
        };

        Ok::<i32, OpfsSAHError>(ret)
    };

    match f() {
        Ok(ret) => ret,
        Err(e) => pool.store_err(&e, Some(SQLITE_IOERR)),
    }
}

unsafe extern "C" fn xAccess(
    pVfs: *mut sqlite3_vfs,
    zName: *const ::std::os::raw::c_char,
    _flags: ::std::os::raw::c_int,
    pResOut: *mut ::std::os::raw::c_int,
) -> ::std::os::raw::c_int {
    let pool = pool(pVfs);
    pool.pop_err();

    *pResOut = match pool
        .get_path(zName)
        .and_then(|path| pool.resolve_sqlite_path(&path))
    {
        Ok(path) => i32::from(path.is_some()),
        Err(error) => return pool.store_err(&error, Some(SQLITE_CANTOPEN)),
    };

    SQLITE_OK
}

unsafe extern "C" fn xDelete(
    pVfs: *mut sqlite3_vfs,
    zName: *const ::std::os::raw::c_char,
    _syncDir: ::std::os::raw::c_int,
) -> ::std::os::raw::c_int {
    let pool = pool(pVfs);
    pool.pop_err();

    if let Err(e) = pool
        .get_path(zName)
        .and_then(|name| pool.delete_sqlite_path(&name))
    {
        return pool.store_err(&e, Some(SQLITE_IOERR_DELETE));
    }

    SQLITE_OK
}

unsafe extern "C" fn xFullPathname(
    _pVfs: *mut sqlite3_vfs,
    zName: *const ::std::os::raw::c_char,
    nOut: ::std::os::raw::c_int,
    zOut: *mut ::std::os::raw::c_char,
) -> ::std::os::raw::c_int {
    if zName.is_null() || zOut.is_null() || nOut <= 0 {
        return SQLITE_CANTOPEN;
    }
    let path = CStr::from_ptr(zName);
    let bytes = path.to_bytes_with_nul();
    if bytes.len() > nOut as usize {
        return SQLITE_CANTOPEN;
    }
    bytes
        .as_ptr()
        .cast::<::std::os::raw::c_char>()
        .copy_to_nonoverlapping(zOut, bytes.len());
    SQLITE_OK
}

unsafe extern "C" fn xGetLastError(
    pVfs: *mut sqlite3_vfs,
    nOut: ::std::os::raw::c_int,
    zOut: *mut ::std::os::raw::c_char,
) -> ::std::os::raw::c_int {
    let pool = pool(pVfs);
    let Some((code, msg)) = pool.pop_err() else {
        return SQLITE_OK;
    };
    if !zOut.is_null() {
        let count = msg.len().min(nOut as usize);
        msg.as_ptr().copy_to(zOut.cast(), count);
        let zero = match count.cmp(&msg.len()) {
            std::cmp::Ordering::Less | std::cmp::Ordering::Equal => nOut as usize,
            std::cmp::Ordering::Greater => msg.len() + 1,
        };
        if zero > 0 {
            std::ptr::write(zOut.add(zero - 1), 0);
        }
    }
    code
}

unsafe extern "C" fn xOpen(
    pVfs: *mut sqlite3_vfs,
    zName: sqlite3_filename,
    pFile: *mut sqlite3_file,
    flags: ::std::os::raw::c_int,
    pOutFlags: *mut ::std::os::raw::c_int,
) -> ::std::os::raw::c_int {
    let pool = pool(pVfs);

    let f = || {
        let name = if zName.is_null() {
            pool.unique_temp_path()?
        } else {
            pool.get_path(zName)?
        };
        if pool.is_path_reservation_claimed(&name)? {
            return Err(OpfsSAHError::Custom(format!(
                "file is reserved by an active import: {name}"
            )));
        }
        let (stored_name, sah) = match pool.resolve_sqlite_path(&name)? {
            Some((stored_name, sah)) => (stored_name, sah),
            None => {
                if flags & SQLITE_OPEN_CREATE == 0 {
                    return Err(OpfsSAHError::Custom(format!("file not found: {name}")));
                }
                if pool.is_path_claimed(&name)? {
                    return Err(OpfsSAHError::Custom(format!(
                        "A compatible database path already exists: {name}"
                    )));
                }
                let stored_name = parent_url_path(&name)?;
                if stored_name.len() >= HEADER_MAX_PATH_SIZE {
                    return Err(OpfsSAHError::Custom(format!(
                        "Path too long: {stored_name}"
                    )));
                }
                if let Some(sah) = pool.next_available_sah() {
                    pool.set_associated_path(&sah, &stored_name, flags)?;
                    (stored_name, sah)
                } else {
                    return Err(OpfsSAHError::Custom(
                        "SAH pool is full. Cannot create file".into(),
                    ));
                }
            }
        };
        let file = Object::new();
        Reflect::set(&file, &JsValue::from("path"), &JsValue::from(stored_name)).unwrap();
        Reflect::set(&file, &JsValue::from("flags"), &JsValue::from(flags)).unwrap();
        Reflect::set(&file, &JsValue::from("sah"), &JsValue::from(sah)).unwrap();
        pool.map_s3_file_to_o_file(pFile, Some(file));

        (*(pFile.cast::<OpfsFile>())).vfs = pVfs;
        (*pFile).pMethods = &IO_METHODS;

        if !pOutFlags.is_null() {
            *pOutFlags = flags;
        }

        Ok::<i32, OpfsSAHError>(SQLITE_OK)
    };
    match f() {
        Ok(ret) => ret,
        Err(e) => pool.store_err(&e, Some(SQLITE_CANTOPEN)),
    }
}

static IO_METHODS: sqlite3_io_methods = sqlite3_io_methods {
    iVersion: 1,
    xClose: Some(xClose),
    xRead: Some(xRead),
    xWrite: Some(xWrite),
    xTruncate: Some(xTruncate),
    xSync: Some(xSync),
    xFileSize: Some(xFileSize),
    xLock: Some(xLock),
    xUnlock: Some(xUnlock),
    xCheckReservedLock: Some(xCheckReservedLock),
    xFileControl: Some(xFileControl),
    xSectorSize: Some(xSectorSize),
    xDeviceCharacteristics: Some(xDeviceCharacteristics),
    xShmMap: None,
    xShmLock: None,
    xShmBarrier: None,
    xShmUnmap: None,
    xFetch: None,
    xUnfetch: None,
};

fn vfs(name: *const ::std::os::raw::c_char) -> sqlite3_vfs {
    let default_vfs = unsafe { sqlite3_vfs_find(std::ptr::null()) };
    let xRandomness = unsafe { (*default_vfs).xRandomness };
    let xSleep = unsafe { (*default_vfs).xSleep };
    let xCurrentTime = unsafe { (*default_vfs).xCurrentTime };
    let xCurrentTimeInt64 = unsafe { (*default_vfs).xCurrentTimeInt64 };

    sqlite3_vfs {
        iVersion: 2,
        szOsFile: std::mem::size_of::<OpfsFile>() as i32,
        mxPathname: HEADER_MAX_PATH_SIZE as i32,
        pNext: std::ptr::null_mut(),
        zName: name,
        pAppData: std::ptr::null_mut(),
        xOpen: Some(xOpen),
        xDelete: Some(xDelete),
        xAccess: Some(xAccess),
        xFullPathname: Some(xFullPathname),
        xDlOpen: None,
        xDlError: None,
        xDlSym: None,
        xDlClose: None,
        xRandomness,
        xSleep,
        xCurrentTime,
        xGetLastError: Some(xGetLastError),
        xCurrentTimeInt64,
        xSetSystemCall: None,
        xGetSystemCall: None,
        xNextSystemCall: None,
    }
}

struct OpfsSAH {
    pool: Arc<FragileComfirmed<OpfsSAHPool>>,
}

impl OpfsSAH {
    fn new(pool: OpfsSAHPool) -> Self {
        Self {
            pool: Arc::new(FragileComfirmed::new(pool)),
        }
    }
}

/// Build `OpfsSAHPoolCfg`
pub struct OpfsSAHPoolCfgBuilder(OpfsSAHPoolCfg);

impl OpfsSAHPoolCfgBuilder {
    pub fn new() -> Self {
        Self(OpfsSAHPoolCfg::default())
    }

    /// The SQLite VFS name under which this pool's VFS is registered.
    pub fn vfs_name(mut self, name: &str) -> Self {
        self.0.vfs_name = name.into();
        self
    }

    /// Specifies the OPFS directory name in which to store metadata for the `vfs_name`
    pub fn directory(mut self, directory: &str) -> Self {
        self.0.directory = directory.into();
        self
    }

    /// If truthy, contents and filename mapping are removed from each SAH
    /// as it is acquired during initalization of the VFS, leaving the VFS's
    /// storage in a pristine state. Use this only for databases which need not
    /// survive a page reload.
    pub fn clear_on_init(mut self, set: bool) -> Self {
        self.0.clear_on_init = set;
        self
    }

    /// Specifies the default capacity of the VFS, i.e. the number of files
    /// it may contain.
    pub fn initial_capacity(mut self, cap: u32) -> Self {
        self.0.initial_capacity = cap;
        self
    }

    /// Build OpfsSAHPoolCfg
    pub fn build(self) -> OpfsSAHPoolCfg {
        self.0
    }
}

impl Default for OpfsSAHPoolCfgBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// `OpfsSAHPool` options
pub struct OpfsSAHPoolCfg {
    /// The SQLite VFS name under which this pool's VFS is registered.
    pub vfs_name: String,
    /// Specifies the OPFS directory name in which to store metadata for the `vfs_name`
    pub directory: String,
    /// If truthy, contents and filename mapping are removed from each SAH
    /// as it is acquired during initalization of the VFS, leaving the VFS's
    /// storage in a pristine state. Use this only for databases which need not
    /// survive a page reload.
    pub clear_on_init: bool,
    /// Specifies the default capacity of the VFS, i.e. the number of files
    /// it may contain.
    pub initial_capacity: u32,
}

impl Default for OpfsSAHPoolCfg {
    fn default() -> Self {
        Self {
            vfs_name: "opfs-sahpool".into(),
            directory: ".opfs-sahpool".into(),
            clear_on_init: false,
            initial_capacity: 6,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum OpfsSAHError {
    #[error("this vfs is only available in workers")]
    NotSuported,
    #[error("get directory handle error")]
    GetDirHandle(JsValue),
    #[error("get file handle error")]
    GetFileHandle(JsValue),
    #[error("create sync access handle error")]
    CreateSyncAccessHandle(JsValue),
    #[error("iterate handle error")]
    IterHandle(JsValue),
    #[error("get path error")]
    GetPath(JsValue),
    #[error("remove entity error")]
    RemoveEntity(JsValue),
    #[error("get size error")]
    GetSize(JsValue),
    #[error("sah read error")]
    Read(JsValue),
    #[error("sah write error")]
    Write(JsValue),
    #[error("sah flush error")]
    Flush(JsValue),
    #[error("sah truncate error")]
    Truncate(JsValue),
    #[error("reflect error")]
    Reflect(JsValue),
    #[error("custom error")]
    Custom(String),
}

/// A OpfsSAHPoolUtil instance is exposed to clients in order to
/// manipulate an OpfsSAHPool object without directly exposing that
/// object and allowing for some semantic changes compared to that
/// class.
pub struct OpfsSAHPoolUtil {
    pool: Arc<FragileComfirmed<OpfsSAHPool>>,
}

/// Incremental SQLite database importer for the OPFS SAH pool.
///
/// This keeps only the caller's current chunk in memory and writes it directly
/// into the pool slot. The database becomes visible under `path` only after
/// [`finish`](Self::finish) validates and commits the import.
pub struct OpfsSAHPoolImport {
    pool: Arc<FragileComfirmed<OpfsSAHPool>>,
    sah: FileSystemSyncAccessHandle,
    path: String,
    reservation: String,
    length: u64,
    header: Vec<u8>,
    active: bool,
}

impl OpfsSAHPoolImport {
    fn cleanup_once(&mut self) -> Result<(), OpfsSAHError> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        self.pool.set_associated_path(&self.sah, "", 0)?;
        self.pool.release_path(&self.reservation);
        Ok(())
    }

    fn fail<T>(&mut self, error: OpfsSAHError) -> Result<T, OpfsSAHError> {
        match self.cleanup_once() {
            Ok(()) => Err(error),
            Err(cleanup) => Err(combined_error(error, cleanup)),
        }
    }

    /// Append one decompressed database-file chunk.
    pub fn write_chunk(&mut self, bytes: &[u8]) -> Result<(), OpfsSAHError> {
        if !self.active {
            return Err(OpfsSAHError::Custom(
                "Cannot write to a finished database import.".into(),
            ));
        }
        if bytes.is_empty() {
            return Ok(());
        }

        if self.header.len() < SQLITE_HEADER_SIZE {
            let needed = SQLITE_HEADER_SIZE - self.header.len();
            self.header
                .extend_from_slice(&bytes[..bytes.len().min(needed)]);
            if self.header.len() == SQLITE_HEADER_SIZE {
                if let Err(error) = sqlite_page_size(&self.header) {
                    return self.fail(error);
                }
            }
        }

        let chunk_length = match u64::try_from(bytes.len()) {
            Ok(length) => length,
            Err(_) => {
                return self.fail(OpfsSAHError::Custom(
                    "SQLite database chunk is too large.".into(),
                ))
            }
        };
        let next_length = match self.length.checked_add(chunk_length) {
            Some(length) => length,
            None => {
                return self.fail(OpfsSAHError::Custom(
                    "SQLite database size overflow.".into(),
                ))
            }
        };
        if let Err(error) = checked_data_offset(next_length) {
            return self.fail(error);
        }
        let offset = match checked_data_offset(self.length) {
            Ok(offset) => offset,
            Err(error) => return self.fail(error),
        };
        let write = match self
            .sah
            .write_with_u8_array_and_options(bytes, &read_write_options(offset))
        {
            Ok(write) => write,
            Err(error) => return self.fail(OpfsSAHError::Write(error)),
        };
        if write != bytes.len() as f64 {
            return self.fail(OpfsSAHError::Custom(format!(
                "Expected to write {} bytes but wrote {}.",
                bytes.len(),
                write
            )));
        }
        self.length = next_length;
        Ok(())
    }

    /// Validate and publish the imported database under its requested path.
    pub fn finish(mut self) -> Result<u64, OpfsSAHError> {
        if !self.active {
            return Err(OpfsSAHError::Custom(
                "Cannot finish an inactive database import.".into(),
            ));
        }
        let page_size = match sqlite_page_size(&self.header) {
            Ok(page_size) => page_size,
            Err(error) => return self.fail(error),
        };
        if let Err(error) = validate_database_length(self.length, page_size) {
            return self.fail(error);
        }

        let truncate_length = match checked_data_offset(self.length) {
            Ok(length) => length,
            Err(error) => return self.fail(error),
        };
        if let Err(error) = self.sah.truncate_with_f64(truncate_length) {
            return self.fail(OpfsSAHError::Truncate(error));
        }
        let journal_mode = [1, 1];
        let written = match self.sah.write_with_u8_array_and_options(
            &journal_mode,
            &read_write_options((HEADER_OFFSET_DATA + 18) as f64),
        ) {
            Ok(written) => written,
            Err(error) => return self.fail(OpfsSAHError::Write(error)),
        };
        if written != journal_mode.len() as f64 {
            return self.fail(OpfsSAHError::Custom(format!(
                "Expected to write {} journal-mode bytes but wrote {}.",
                journal_mode.len(),
                written
            )));
        }
        if let Err(error) = self.sah.flush() {
            return self.fail(OpfsSAHError::Flush(error));
        }
        if let Err(error) =
            self.pool
                .set_associated_path(&self.sah, &self.path, SQLITE_OPEN_MAIN_DB)
        {
            return self.fail(error);
        }
        self.active = false;
        self.pool.release_path(&self.reservation);
        Ok(self.length)
    }

    /// Discard a partial import and return its pool slot for reuse.
    pub fn abort(mut self) -> Result<(), OpfsSAHError> {
        self.cleanup_once()
    }
}

impl Drop for OpfsSAHPoolImport {
    fn drop(&mut self) {
        let _ = self.cleanup_once();
    }
}

impl OpfsSAHPoolUtil {
    /// Adds n entries to the current pool.
    pub async fn add_capacity(&self, n: u32) -> Result<u32, OpfsSAHError> {
        self.pool.add_capacity(n).await
    }

    /// Removes up to n entries from the pool, with the caveat that
    /// it can only remove currently-unused entries.
    pub async fn reduce_capacity(&self, n: u32) -> Result<u32, OpfsSAHError> {
        self.pool.reduce_capacity(n).await
    }

    /// Returns the number of files currently contained in the SAH pool.
    pub fn get_capacity(&self) -> u32 {
        self.pool.get_capacity()
    }

    /// Returns the number of files from the pool currently allocated to VFS slots.
    pub fn get_file_count(&self) -> u32 {
        self.pool.get_file_count()
    }

    /// Returns an array of the names of the files currently allocated to VFS slots.
    pub fn get_file_names(&self) -> Vec<String> {
        self.pool.get_file_names()
    }

    /// Returns whether a logical path is currently associated with a pool
    /// entry.
    pub fn has_path(&self, path: &str) -> bool {
        self.pool
            .resolve_utility_path(path)
            .is_ok_and(|path| path.is_some())
    }

    /// Reassociate a closed logical path with a new logical path without
    /// copying its database bytes. The source must not be open through SQLite.
    pub fn rename_path(&self, from: &str, to: &str) -> Result<(), OpfsSAHError> {
        if from.is_empty() || to.is_empty() {
            return Err(OpfsSAHError::Custom(
                "Source and destination paths must not be empty.".into(),
            ));
        }
        let to_path = new_utility_path(to)?;

        let Some((stored_from, sah)) = self.pool.resolve_utility_path(from)? else {
            return Err(OpfsSAHError::Custom(format!(
                "Source path does not exist: {from}"
            )));
        };
        if self.pool.is_path_open(from)? {
            return Err(OpfsSAHError::Custom(format!(
                "Source path is open and cannot be renamed: {from}"
            )));
        }
        self.pool.reserve_path(to)?;
        if let Err(error) = self
            .pool
            .set_associated_path(&sah, &to_path, SQLITE_OPEN_MAIN_DB)
        {
            return match self.pool.restore_legacy_associated_path(
                &sah,
                &stored_from,
                SQLITE_OPEN_MAIN_DB,
            ) {
                Ok(()) => {
                    self.pool.release_path(to);
                    Err(error)
                }
                Err(rollback) => {
                    self.pool.quarantine_path(&stored_from);
                    self.pool
                        .map_filename_to_sah
                        .delete(&JsValue::from(&stored_from));
                    Err(combined_error(error, rollback))
                }
            };
        }
        self.pool
            .map_filename_to_sah
            .delete(&JsValue::from(stored_from));
        self.pool.release_path(to);
        Ok(())
    }

    /// Removes up to n entries from the pool, with the caveat that it can only
    /// remove currently-unused entries.
    pub async fn reserve_minimum_capacity(&self, min: u32) -> Result<(), OpfsSAHError> {
        let now = self.pool.get_capacity();
        if min > now {
            self.pool.add_capacity(min - now).await?;
        }
        Ok(())
    }

    /// If a virtual file exists with the given name, disassociates it
    /// from the pool and returns true, else returns false without side effects.
    pub fn unlink(&self, name: &str) -> Result<bool, OpfsSAHError> {
        self.pool.delete_path(name)
    }

    /// Synchronously reads the contents of the given file into a Uint8Array and returns it.
    pub fn export_file(&self, name: &str) -> Result<Vec<u8>, OpfsSAHError> {
        self.pool.export_file(name)
    }

    /// Imports an SQLite database into a new logical path.
    /// Existing or actively-imported destinations are rejected.
    pub fn import_db(&self, path: &str, bytes: &[u8]) -> Result<(), OpfsSAHError> {
        let stored_path = new_utility_path(path)?;
        self.pool.import_db(&stored_path, path, bytes)
    }

    /// Begin a bounded-memory import into an unused pool slot.
    /// Existing destinations must be activated separately after this import
    /// has finished so a failed stream cannot destroy the current database.
    /// The target database must not be open while the import is active.
    pub fn begin_import_db(&self, path: &str) -> Result<OpfsSAHPoolImport, OpfsSAHError> {
        let stored_path = new_utility_path(path)?;
        self.pool.reserve_path(path)?;

        let sah = match self.pool.next_available_sah() {
            Some(sah) => sah,
            None => {
                self.pool.release_path(path);
                return Err(OpfsSAHError::Custom(
                    "No available handles to import to.".into(),
                ));
            }
        };
        self.pool.available_sah.delete(&sah);
        if let Err(error) = sah.truncate_with_u32(HEADER_OFFSET_DATA as u32) {
            self.pool.available_sah.add(&sah);
            self.pool.release_path(path);
            return Err(OpfsSAHError::Truncate(error));
        }

        Ok(OpfsSAHPoolImport {
            pool: Arc::clone(&self.pool),
            sah,
            path: stored_path,
            reservation: path.to_owned(),
            length: 0,
            header: Vec::with_capacity(SQLITE_HEADER_SIZE),
            active: true,
        })
    }

    /// Clears all client-defined state of all SAHs and makes all of them available
    /// for re-use by the pool.
    pub async fn wipe_files(&self) -> Result<(), OpfsSAHError> {
        if self.pool.reserved_paths.size() != 0 {
            return Err(OpfsSAHError::Custom(
                "Cannot wipe files while a database import is active.".into(),
            ));
        }
        if self.pool.map_s3_file_to_o_file.size() != 0 {
            return Err(OpfsSAHError::Custom(
                "Cannot wipe files while SQLite files are open.".into(),
            ));
        }
        self.pool.release_access_handles();
        self.pool.acquire_access_handles(true).await?;
        Ok(())
    }
}

/// Register `opfs-sahpool` vfs and return a utility object which can be used
/// to perform basic administration of the file pool
pub async fn install_opfs_sahpool(
    options: Option<&OpfsSAHPoolCfg>,
    default_vfs: bool,
) -> Result<OpfsSAHPoolUtil, OpfsSAHError> {
    let default_options = OpfsSAHPoolCfg::default();
    let options = options.unwrap_or(&default_options);
    let vfs_name = &options.vfs_name;

    let create_pool = async {
        let pool = OpfsSAHPool::new(options).await?;
        Ok(OpfsSAH::new(pool))
    };

    let register_vfs = || {
        let name =
            CString::new(vfs_name.clone()).map_err(|e| OpfsSAHError::Custom(format!("{e:?}")))?;
        let vfs = Box::leak(Box::new(vfs(name.into_raw())));

        let ret = unsafe { sqlite3_vfs_register(vfs, i32::from(default_vfs)) };
        if ret != SQLITE_OK {
            unsafe {
                drop(Box::from_raw(vfs));
            }
            return Err(OpfsSAHError::Custom(format!(
                "register {vfs_name} vfs failed",
            )));
        }

        Ok(vfs as *mut sqlite3_vfs)
    };

    static NAME2VFS: Lazy<tokio::sync::Mutex<HashMap<String, Arc<OpfsSAH>>>> =
        Lazy::new(|| tokio::sync::Mutex::new(HashMap::new()));

    let mut name2vfs = NAME2VFS.lock().await;

    let pool = if let Some(sah) = name2vfs.get(vfs_name) {
        Arc::clone(&sah.pool)
    } else {
        let opfs_sah = Arc::new(create_pool.await?);
        let vfs = register_vfs()?;
        name2vfs.insert(vfs_name.clone(), Arc::clone(&opfs_sah));
        VFS2SAH.write().insert(vfs as usize, Arc::clone(&opfs_sah));
        Arc::clone(&opfs_sah.pool)
    };

    let util = OpfsSAHPoolUtil { pool };

    Ok(util)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_worker);

    const UNSTABLE_UTILITY_PATHS: &[&str] = &[
        "snapshot:v1.db",
        "snapshot:/v1.db",
        "https:orders.db",
        "https://example.com/orders.db",
        "file:orders.db",
        "file:/orders.db",
        "//host/v1.db",
        " snapshot:/v1.db",
        "snapshot:/v1.db ",
        "\u{0001}snapshot:/v1.db",
        "snap\tshot:/v1.db",
        "snap\rshot:/v1.db",
        "snap\nshot:/v1.db",
        "\t//host/v1.db",
        r"\\host\v1.db",
        r"file:\host\v1.db",
    ];

    fn sqlite_header(encoded_page_size: u16) -> [u8; SQLITE_HEADER_SIZE] {
        let mut header = [0; SQLITE_HEADER_SIZE];
        header[..16].copy_from_slice(b"SQLite format 3\0");
        header[16..18].copy_from_slice(&encoded_page_size.to_be_bytes());
        header
    }

    #[wasm_bindgen_test]
    fn parses_supported_sqlite_page_sizes() {
        assert_eq!(sqlite_page_size(&sqlite_header(512)).unwrap(), 512);
        assert_eq!(sqlite_page_size(&sqlite_header(4096)).unwrap(), 4096);
        assert_eq!(sqlite_page_size(&sqlite_header(1)).unwrap(), 65_536);
    }

    #[wasm_bindgen_test]
    fn rejects_invalid_sqlite_page_sizes_and_lengths() {
        assert!(sqlite_page_size(&sqlite_header(0)).is_err());
        assert!(sqlite_page_size(&sqlite_header(1000)).is_err());
        assert!(validate_database_length(4097, 4096).is_err());
        assert!(validate_database_length(4096, 4096).is_ok());
    }

    #[wasm_bindgen_test]
    fn rejects_offsets_outside_the_javascript_safe_integer_range() {
        assert!(checked_data_offset(JS_MAX_SAFE_INTEGER - HEADER_OFFSET_DATA as u64).is_ok());
        assert!(checked_data_offset(JS_MAX_SAFE_INTEGER).is_err());
        assert!(checked_data_offset(u64::MAX).is_err());
    }

    #[wasm_bindgen_test]
    fn canonical_paths_preserve_parent_format_names() {
        assert_eq!(
            canonicalize_path("café db.sqlite").unwrap(),
            "/café db.sqlite"
        );
        assert_eq!(
            canonicalize_path("//folder/./child/../café db.sqlite").unwrap(),
            "/folder/café db.sqlite"
        );
        assert_eq!(
            canonicalize_path(&canonicalize_path("../../café db.sqlite").unwrap()).unwrap(),
            "/café db.sqlite"
        );
        assert!(canonicalize_path("https://example.com/db").is_err());
        assert!(canonicalize_path("").is_err());
        assert!(canonicalize_path("bad\0path").is_err());
        assert_eq!(
            path_identities("café db.sqlite").unwrap(),
            vec![
                "café db.sqlite",
                "/café db.sqlite",
                "/caf%C3%A9%20db.sqlite"
            ]
        );
        assert_eq!(
            parent_url_path("opfs-sahpool:orders.db").unwrap(),
            "orders.db"
        );
        assert!(paths_overlap("/café.db", "/caf%C3%A9.db").unwrap());
        for &unsafe_name in UNSTABLE_UTILITY_PATHS {
            assert!(new_utility_path(unsafe_name).is_err(), "{unsafe_name}");
        }
        assert_eq!(
            new_utility_path("/snapshot:v1.db").unwrap(),
            "/snapshot:v1.db"
        );
        for stable_name in ["orders.db", "/snapshot:v1.db", "folder/../orders.db"] {
            let published = new_utility_path(stable_name).unwrap();
            assert_eq!(
                parent_url_path(stable_name).unwrap(),
                parent_url_path(&published).unwrap()
            );
        }
    }

    fn sqlite_database_bytes() -> Vec<u8> {
        let mut bytes = vec![0; 512];
        bytes[..16].copy_from_slice(b"SQLite format 3\0");
        bytes[16..18].copy_from_slice(&512u16.to_be_bytes());
        bytes[18] = 1;
        bytes[19] = 1;
        bytes
    }

    fn open_test_database(config: &OpfsSAHPoolCfg, name: &str, flags: i32) -> *mut sqlite3 {
        let name = CString::new(name).unwrap();
        let vfs_name = CString::new(config.vfs_name.as_str()).unwrap();
        let mut db = std::ptr::null_mut();
        let result = unsafe { sqlite3_open_v2(name.as_ptr(), &mut db, flags, vfs_name.as_ptr()) };
        assert_eq!(result, SQLITE_OK, "database {name:?} must open");
        db
    }

    fn exec_test_sql(db: *mut sqlite3, sql: &str) {
        let sql = CString::new(sql).unwrap();
        assert_eq!(
            unsafe {
                sqlite3_exec(
                    db,
                    sql.as_ptr(),
                    None,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            SQLITE_OK
        );
    }

    fn query_test_integer(db: *mut sqlite3, sql: &str) -> i32 {
        let sql = CString::new(sql).unwrap();
        let mut statement = std::ptr::null_mut();
        assert_eq!(
            unsafe {
                sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut statement, std::ptr::null_mut())
            },
            SQLITE_OK
        );
        assert_eq!(unsafe { sqlite3_step(statement) }, SQLITE_ROW);
        let value = unsafe { sqlite3_column_int(statement, 0) };
        assert_eq!(unsafe { sqlite3_finalize(statement) }, SQLITE_OK);
        value
    }

    fn create_database_bytes(
        util: &OpfsSAHPoolUtil,
        config: &OpfsSAHPoolCfg,
        path: &str,
        value: i32,
    ) -> Vec<u8> {
        let db = open_test_database(config, path, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE);
        exec_test_sql(
            db,
            &format!("CREATE TABLE snapshot_value(value INTEGER); INSERT INTO snapshot_value VALUES ({value})"),
        );
        assert_eq!(unsafe { sqlite3_close(db) }, SQLITE_OK);
        let bytes = util.export_file(path).unwrap();
        assert!(util.unlink(path).unwrap());
        bytes
    }

    fn test_pool_config(capacity: u32) -> OpfsSAHPoolCfg {
        let suffix = get_random_name();
        OpfsSAHPoolCfgBuilder::new()
            .vfs_name(&format!("opfs-test-{suffix}"))
            .directory(&format!(".opfs-test-{suffix}"))
            .initial_capacity(capacity)
            .build()
    }

    fn rewrite_as_legacy_metadata(util: &OpfsSAHPoolUtil, current_path: &str, legacy_path: &str) {
        let sah = util
            .pool
            .map_filename_to_sah
            .get(&JsValue::from(current_path));
        assert!(!sah.is_undefined(), "seed database must exist");
        let sah = FileSystemSyncAccessHandle::from(sah);

        util.pool
            .restore_legacy_associated_path(&sah, legacy_path, SQLITE_OPEN_MAIN_DB)
            .expect("legacy path seed must be written");
        util.pool
            .map_filename_to_sah
            .delete(&JsValue::from(current_path));

        // sqlite-wasm-rs 0.3.0 metadata used the same body with no V2 marker
        // and a zero digest. Rewrite both pieces to exercise that exact format.
        util.pool
            .dv_body
            .set_uint32(HEADER_OFFSET_FLAGS, SQLITE_OPEN_MAIN_DB as u32);
        let digest = compute_digest(&util.pool.ap_body, SQLITE_OPEN_MAIN_DB as u32);
        let body_written = sah
            .write_with_js_u8_array_and_options(&util.pool.ap_body, &read_write_options(0.0))
            .expect("legacy metadata body write must succeed");
        assert_eq!(body_written, util.pool.ap_body.byte_length() as f64);
        let digest_written = sah
            .write_with_buffer_source_and_options(
                &digest,
                &read_write_options(HEADER_OFFSET_DIGEST as f64),
            )
            .expect("legacy metadata digest write must succeed");
        assert_eq!(digest_written, digest.byte_length() as f64);
        sah.flush().expect("legacy metadata must be durable");
    }

    async fn persisted_opaque_size(pool: &OpfsSAHPool, name: &str) -> f64 {
        let options = FileSystemGetFileOptions::new();
        let handle: FileSystemFileHandle =
            JsFuture::from(pool.dh_opaque.get_file_handle_with_options(name, &options))
                .await
                .unwrap()
                .into();
        let sah: FileSystemSyncAccessHandle = JsFuture::from(handle.create_sync_access_handle())
            .await
            .unwrap()
            .into();
        let size = sah.get_size().unwrap();
        sah.close();
        size
    }

    async fn assert_short_metadata_reload_is_non_destructive(truncated_size: u32) {
        let config = test_pool_config(2);
        let util = install_opfs_sahpool(Some(&config), false).await.unwrap();
        util.import_db("legacy-seed.db", &sqlite_database_bytes())
            .unwrap();
        rewrite_as_legacy_metadata(&util, "/legacy-seed.db", "/legacy.db");
        let legacy_sah = FileSystemSyncAccessHandle::from(
            util.pool
                .map_filename_to_sah
                .get(&JsValue::from("/legacy.db")),
        );
        let legacy_name = util
            .pool
            .map_sah_to_name
            .get(&legacy_sah)
            .as_string()
            .unwrap();
        let legacy_size = legacy_sah.get_size().unwrap();

        let malformed = util.pool.next_available_sah().unwrap();
        let malformed_name = util
            .pool
            .map_sah_to_name
            .get(&malformed)
            .as_string()
            .unwrap();
        malformed.truncate_with_u32(truncated_size).unwrap();
        malformed.flush().unwrap();

        util.pool.release_access_handles();
        assert!(util.pool.acquire_access_handles(false).await.is_err());
        assert_eq!(
            persisted_opaque_size(&util.pool, &malformed_name).await,
            truncated_size as f64
        );
        assert_eq!(
            persisted_opaque_size(&util.pool, &legacy_name).await,
            legacy_size
        );
    }

    #[wasm_bindgen_test]
    async fn short_metadata_body_fails_reload_without_mutation() {
        assert_short_metadata_reload_is_non_destructive((HEADER_CORPUS_SIZE - 1) as u32).await;
    }

    #[wasm_bindgen_test]
    async fn short_metadata_digest_fails_reload_without_mutation() {
        assert_short_metadata_reload_is_non_destructive(
            (HEADER_CORPUS_SIZE + HEADER_DIGEST_SIZE - 1) as u32,
        )
        .await;
    }

    #[wasm_bindgen_test]
    async fn streaming_import_reserves_path_and_capacity_then_reuses_aborted_slot() {
        let config = test_pool_config(1);
        let util = install_opfs_sahpool(Some(&config), false)
            .await
            .expect("OPFS pool installation must succeed");

        let first = util
            .begin_import_db("snapshot.db")
            .expect("first import must reserve a slot");
        assert!(util.begin_import_db("/snapshot.db").is_err());
        assert!(util.begin_import_db("other.db").is_err());

        first.abort().expect("abort must durably return the slot");
        util.begin_import_db("other.db")
            .expect("aborted slot must be reusable")
            .abort()
            .expect("second abort must succeed");
    }

    #[wasm_bindgen_test]
    async fn parent_scheme_name_reopens_existing_database_without_allocating() {
        let config = test_pool_config(2);
        let util = install_opfs_sahpool(Some(&config), false)
            .await
            .expect("OPFS pool installation must succeed");
        let bytes = sqlite_database_bytes();
        util.import_db("seed.db", &bytes).unwrap();
        rewrite_as_legacy_metadata(&util, "/seed.db", "orders.db");
        util.pool.release_access_handles();
        util.pool.acquire_access_handles(false).await.unwrap();

        let name = CString::new("opfs-sahpool:orders.db").unwrap();
        let vfs_name = CString::new(config.vfs_name.as_str()).unwrap();
        let mut db = std::ptr::null_mut();
        assert_eq!(
            unsafe {
                sqlite3_open_v2(
                    name.as_ptr(),
                    &mut db,
                    SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
                    vfs_name.as_ptr(),
                )
            },
            SQLITE_OK
        );
        assert_eq!(unsafe { sqlite3_close(db) }, SQLITE_OK);
        assert_eq!(util.get_file_count(), 1);
        assert_eq!(util.export_file("opfs-sahpool:orders.db").unwrap(), bytes);
    }

    #[wasm_bindgen_test]
    async fn scheme_open_reads_canonical_utility_snapshot_after_reload() {
        let config = test_pool_config(3);
        let util = install_opfs_sahpool(Some(&config), false).await.unwrap();
        let bytes = create_database_bytes(&util, &config, "/source.db", 42);
        util.import_db("/snapshot.db", &bytes).unwrap();
        util.pool.release_access_handles();
        util.pool.acquire_access_handles(false).await.unwrap();

        let db = open_test_database(
            &config,
            "opfs-sahpool:snapshot.db",
            SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
        );
        assert_eq!(
            query_test_integer(db, "SELECT value FROM snapshot_value"),
            42
        );
        assert_eq!(unsafe { sqlite3_close(db) }, SQLITE_OK);
        assert_eq!(util.get_file_count(), 1);
    }

    #[wasm_bindgen_test]
    async fn scheme_open_prefers_legacy_exact_when_both_identities_exist() {
        let config = test_pool_config(4);
        let util = install_opfs_sahpool(Some(&config), false).await.unwrap();
        let canonical_bytes = create_database_bytes(&util, &config, "/canonical-seed.db", 22);
        let legacy_bytes = create_database_bytes(&util, &config, "/legacy-source.db", 11);
        util.import_db("/shared.db", &canonical_bytes).unwrap();
        util.import_db("/legacy-seed.db", &legacy_bytes).unwrap();
        rewrite_as_legacy_metadata(&util, "/legacy-seed.db", "shared.db");
        util.pool.release_access_handles();
        util.pool.acquire_access_handles(false).await.unwrap();

        let legacy = open_test_database(
            &config,
            "opfs-sahpool:shared.db",
            SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
        );
        assert_eq!(
            query_test_integer(legacy, "SELECT value FROM snapshot_value"),
            11
        );
        assert_eq!(unsafe { sqlite3_close(legacy) }, SQLITE_OK);

        let canonical = open_test_database(&config, "/shared.db", SQLITE_OPEN_READWRITE);
        assert_eq!(
            query_test_integer(canonical, "SELECT value FROM snapshot_value"),
            22
        );
        assert_eq!(unsafe { sqlite3_close(canonical) }, SQLITE_OK);
        assert_eq!(util.get_file_count(), 2);
    }

    #[wasm_bindgen_test]
    async fn compatible_reservations_and_published_paths_block_bypass_spellings() {
        let config = test_pool_config(2);
        let util = install_opfs_sahpool(Some(&config), false)
            .await
            .expect("OPFS pool installation must succeed");
        let pending = util.begin_import_db("/caf%C3%A9.db").unwrap();
        let name = CString::new("/café.db").unwrap();
        let vfs_name = CString::new(config.vfs_name.as_str()).unwrap();
        let mut db = std::ptr::null_mut();
        assert_ne!(
            unsafe {
                sqlite3_open_v2(
                    name.as_ptr(),
                    &mut db,
                    SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
                    vfs_name.as_ptr(),
                )
            },
            SQLITE_OK
        );
        if !db.is_null() {
            unsafe { sqlite3_close(db) };
        }
        assert_eq!(util.get_file_count(), 0);
        pending.abort().unwrap();

        let bytes = sqlite_database_bytes();
        util.import_db("café.db", &bytes).unwrap();
        assert!(util.import_db("caf%C3%A9.db", &bytes).is_err());
        assert!(util.rename_path("café.db", "caf%C3%A9.db").is_err());
        assert_eq!(util.get_file_count(), 1);
        assert_eq!(util.export_file("café.db").unwrap(), bytes);
    }

    #[wasm_bindgen_test]
    async fn parent_maximum_length_path_can_be_exported_and_migrated() {
        let config = test_pool_config(1);
        let util = install_opfs_sahpool(Some(&config), false).await.unwrap();
        let bytes = sqlite_database_bytes();
        util.import_db("seed.db", &bytes).unwrap();
        let legacy_path = format!("/{}", "a".repeat(HEADER_MAX_PATH_SIZE - 1));
        assert_eq!(legacy_path.len(), HEADER_MAX_PATH_SIZE);
        rewrite_as_legacy_metadata(&util, "/seed.db", &legacy_path);
        util.pool.release_access_handles();
        util.pool.acquire_access_handles(false).await.unwrap();

        assert_eq!(util.export_file(&legacy_path).unwrap(), bytes);
        assert!(util.import_db(&legacy_path, &bytes).is_err());
        util.rename_path(&legacy_path, "migrated.db").unwrap();
        assert_eq!(util.export_file("migrated.db").unwrap(), bytes);
    }

    #[wasm_bindgen_test]
    async fn unicode_and_space_paths_round_trip_across_metadata_reload() {
        let config = test_pool_config(2);
        let util = install_opfs_sahpool(Some(&config), false)
            .await
            .expect("OPFS pool installation must succeed");
        let bytes = sqlite_database_bytes();

        util.import_db("encoded-seed.db", &bytes)
            .expect("relative unicode import must succeed");
        util.import_db("literal-seed.db", &bytes)
            .expect("literal seed import must succeed");
        rewrite_as_legacy_metadata(&util, "/encoded-seed.db", "/caf%C3%A9%20legacy.db");
        rewrite_as_legacy_metadata(&util, "/literal-seed.db", "/literal legacy db.sqlite");

        util.pool.release_access_handles();
        util.pool
            .acquire_access_handles(false)
            .await
            .expect("metadata reload must succeed");

        assert!(util.has_path("/café legacy.db"));
        assert_eq!(
            util.export_file("./café legacy.db").unwrap(),
            bytes,
            "legacy URL-normalized metadata must remain accessible by its original path"
        );
        assert_eq!(
            util.export_file("literal legacy db.sqlite").unwrap(),
            bytes,
            "legacy literal importer metadata must remain accessible"
        );

        let legacy_name = CString::new("/café legacy.db").unwrap();
        let vfs_name = CString::new(config.vfs_name.as_str()).unwrap();
        let mut db = std::ptr::null_mut();
        assert_eq!(
            unsafe {
                sqlite3_open_v2(
                    legacy_name.as_ptr(),
                    &mut db,
                    SQLITE_OPEN_READWRITE,
                    vfs_name.as_ptr(),
                )
            },
            SQLITE_OK,
            "SQLite must resolve the original literal name to URL-normalized legacy metadata"
        );
        assert_eq!(unsafe { sqlite3_close(db) }, SQLITE_OK);

        assert!(util
            .unlink("nested/../café legacy.db")
            .expect("unlink must succeed"));
        assert!(!util.has_path("/café legacy.db"));
        assert!(util
            .unlink("literal legacy db.sqlite")
            .expect("literal unlink must succeed"));
    }

    #[wasm_bindgen_test]
    async fn distinct_legacy_alias_entries_remain_addressable_without_new_collisions() {
        let config = test_pool_config(2);
        let util = install_opfs_sahpool(Some(&config), false)
            .await
            .expect("OPFS pool installation must succeed");
        let mut encoded_bytes = sqlite_database_bytes();
        encoded_bytes[100] = 17;
        let mut literal_bytes = sqlite_database_bytes();
        literal_bytes[100] = 29;
        util.import_db("encoded-seed.db", &encoded_bytes).unwrap();
        util.import_db("literal-seed.db", &literal_bytes).unwrap();
        rewrite_as_legacy_metadata(&util, "/encoded-seed.db", "/caf%C3%A9%20db.sqlite");
        rewrite_as_legacy_metadata(&util, "/literal-seed.db", "/café db.sqlite");

        util.pool.release_access_handles();
        util.pool
            .acquire_access_handles(false)
            .await
            .expect("both legacy metadata entries must reload");

        assert_eq!(util.export_file("café db.sqlite").unwrap(), literal_bytes);
        assert_eq!(
            util.export_file("caf%C3%A9%20db.sqlite").unwrap(),
            encoded_bytes
        );
        assert_eq!(util.get_file_count(), 2);
        assert!(util.begin_import_db("café db.sqlite").is_err());
        assert!(util.begin_import_db("caf%C3%A9%20db.sqlite").is_err());
    }

    #[wasm_bindgen_test]
    async fn exact_parent_dot_segment_names_select_the_requested_database() {
        let config = test_pool_config(2);
        let util = install_opfs_sahpool(Some(&config), false).await.unwrap();
        let mut exact_bytes = sqlite_database_bytes();
        exact_bytes[100] = 41;
        let mut normalized_bytes = sqlite_database_bytes();
        normalized_bytes[100] = 73;
        util.import_db("exact-seed.db", &exact_bytes).unwrap();
        util.import_db("normalized-seed.db", &normalized_bytes)
            .unwrap();
        rewrite_as_legacy_metadata(&util, "/exact-seed.db", "/dir/../db.sqlite");
        rewrite_as_legacy_metadata(&util, "/normalized-seed.db", "/db.sqlite");
        util.pool.release_access_handles();
        util.pool.acquire_access_handles(false).await.unwrap();

        assert_eq!(util.export_file("/dir/../db.sqlite").unwrap(), exact_bytes);
        assert_eq!(util.export_file("/db.sqlite").unwrap(), normalized_bytes);
        util.rename_path("/dir/../db.sqlite", "/migrated.db")
            .unwrap();
        assert_eq!(util.export_file("/migrated.db").unwrap(), exact_bytes);
        assert_eq!(util.export_file("/db.sqlite").unwrap(), normalized_bytes);
        assert!(util.unlink("/db.sqlite").unwrap());
        assert_eq!(util.export_file("/migrated.db").unwrap(), exact_bytes);
    }

    #[wasm_bindgen_test]
    async fn utility_scheme_destinations_require_explicit_filesystem_paths() {
        let config = test_pool_config(3);
        let util = install_opfs_sahpool(Some(&config), false).await.unwrap();
        let bytes = sqlite_database_bytes();

        util.import_db("source.db", &bytes).unwrap();
        for &unsafe_name in UNSTABLE_UTILITY_PATHS {
            assert!(
                util.import_db(unsafe_name, &bytes).is_err(),
                "{unsafe_name}"
            );
            assert!(util.begin_import_db(unsafe_name).is_err(), "{unsafe_name}");
            assert!(
                util.rename_path("source.db", unsafe_name).is_err(),
                "{unsafe_name}"
            );
        }
        assert_eq!(util.export_file("source.db").unwrap(), bytes);
        assert!(util.unlink("source.db").unwrap());

        util.import_db("/snapshot:v1.db", &bytes).unwrap();
        assert!(util.import_db("other:v1.db", &bytes).is_err());
        assert!(util.begin_import_db("other:v1.db").is_err());

        let other = CString::new("other:v1.db").unwrap();
        let vfs_name = CString::new(config.vfs_name.as_str()).unwrap();
        let mut db = std::ptr::null_mut();
        assert_eq!(
            unsafe {
                sqlite3_open_v2(
                    other.as_ptr(),
                    &mut db,
                    SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
                    vfs_name.as_ptr(),
                )
            },
            SQLITE_OK
        );
        assert_eq!(unsafe { sqlite3_close(db) }, SQLITE_OK);
        assert_eq!(util.export_file("/snapshot:v1.db").unwrap(), bytes);

        util.pool.release_access_handles();
        util.pool.acquire_access_handles(false).await.unwrap();
        assert_eq!(util.export_file("/snapshot:v1.db").unwrap(), bytes);
        assert!(util.import_db("other:v1.db", &bytes).is_err());
        assert_eq!(util.get_file_count(), 2);
    }

    #[wasm_bindgen_test]
    async fn corrupt_destructive_flags_do_not_modify_database_bytes() {
        let config = test_pool_config(1);
        let util = install_opfs_sahpool(Some(&config), false)
            .await
            .expect("OPFS pool installation must succeed");
        let bytes = sqlite_database_bytes();
        util.import_db("protected.db", &bytes).unwrap();
        let sah = FileSystemSyncAccessHandle::from(
            util.pool
                .map_filename_to_sah
                .get(&JsValue::from("/protected.db")),
        );
        let size_before = sah.get_size().unwrap();

        sah.read_with_buffer_source_and_options(&util.pool.ap_body, &read_write_options(0.0))
            .unwrap();
        util.pool
            .dv_body
            .set_uint32(HEADER_OFFSET_FLAGS, SQLITE_OPEN_DELETEONCLOSE as u32);
        sah.write_with_js_u8_array_and_options(&util.pool.ap_body, &read_write_options(0.0))
            .unwrap();

        assert!(util.pool.get_associated_path(&sah).is_err());
        assert_eq!(sah.get_size().unwrap(), size_before);
        assert_eq!(util.export_file("protected.db").unwrap(), bytes);

        util.pool
            .set_associated_path(&sah, "/protected.db", SQLITE_OPEN_MAIN_DB)
            .expect("test metadata restoration must succeed");
    }

    #[wasm_bindgen_test]
    async fn rename_rejects_reserved_target_and_open_source() {
        let config = test_pool_config(2);
        let util = install_opfs_sahpool(Some(&config), false)
            .await
            .expect("OPFS pool installation must succeed");
        util.import_db("source.db", &sqlite_database_bytes())
            .expect("source import must succeed");

        let target = util
            .begin_import_db("target.db")
            .expect("target reservation must succeed");
        assert!(util.rename_path("source.db", "target.db").is_err());
        target.abort().expect("target abort must succeed");

        let source = CString::new("/source.db").unwrap();
        let vfs_name = CString::new(config.vfs_name.as_str()).unwrap();
        let mut db = std::ptr::null_mut();
        let result = unsafe {
            sqlite3_open_v2(
                source.as_ptr(),
                &mut db,
                SQLITE_OPEN_READWRITE,
                vfs_name.as_ptr(),
            )
        };
        assert_eq!(result, SQLITE_OK, "source database must open");
        assert!(util.rename_path("source.db", "renamed.db").is_err());
        assert!(util.unlink("source.db").is_err());
        assert!(util.wipe_files().await.is_err());
        assert_eq!(unsafe { sqlite3_close(db) }, SQLITE_OK);

        util.rename_path("source.db", "renamed.db")
            .expect("closed source must rename");
        assert!(util.has_path("renamed.db"));
        assert!(util
            .unlink("renamed.db")
            .expect("closed renamed path must unlink"));
        util.wipe_files()
            .await
            .expect("pool with no open SQLite files must wipe");
    }

    #[wasm_bindgen_test]
    async fn failed_rename_rollback_quarantines_both_names_and_preserves_bytes() {
        let config = test_pool_config(2);
        let util = install_opfs_sahpool(Some(&config), false).await.unwrap();
        let bytes = sqlite_database_bytes();
        util.import_db("source.db", &bytes).unwrap();
        let sah = FileSystemSyncAccessHandle::from(
            util.pool
                .map_filename_to_sah
                .get(&JsValue::from("/source.db")),
        );
        *util.pool.association_failures_after_body.lock() = 2;

        assert!(util.rename_path("source.db", "target.db").is_err());
        assert!(!util.has_path("source.db"));
        assert!(!util.has_path("target.db"));
        assert!(util.begin_import_db("source.db").is_err());
        assert!(util.begin_import_db("target.db").is_err());
        assert!(util.unlink("source.db").is_err());
        assert!(util.unlink("target.db").is_err());

        let mut stored = vec![0; bytes.len()];
        assert_eq!(
            sah.read_with_u8_array_and_options(
                &mut stored,
                &read_write_options(HEADER_OFFSET_DATA as f64),
            )
            .unwrap(),
            bytes.len() as f64
        );
        assert_eq!(stored, bytes);
    }

    #[wasm_bindgen_test]
    async fn null_filename_temporary_database_is_deleted_on_close() {
        let config = test_pool_config(1);
        let util = install_opfs_sahpool(Some(&config), false)
            .await
            .expect("OPFS pool installation must succeed");
        let vfs_name = CString::new(config.vfs_name.as_str()).unwrap();
        let vfs = unsafe { sqlite3_vfs_find(vfs_name.as_ptr()) };
        assert!(!vfs.is_null());
        let mut file: OpfsFile = unsafe { std::mem::zeroed() };

        let result = unsafe {
            xOpen(
                vfs,
                std::ptr::null(),
                (&mut file as *mut OpfsFile).cast(),
                SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_DELETEONCLOSE,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, SQLITE_OK, "temporary database must open");
        assert_eq!(util.get_file_count(), 1);
        assert_eq!(
            unsafe { xClose((&mut file as *mut OpfsFile).cast()) },
            SQLITE_OK
        );
        assert_eq!(util.get_file_count(), 0);
    }
}
