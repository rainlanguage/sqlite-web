use super::*;

pub(crate) fn sqlite_transient() -> Option<unsafe extern "C" fn(*mut std::ffi::c_void)> {
    // SQLite uses the value -1 cast to a function pointer as a sentinel meaning
    // "make your own copy" for the result buffer (a.k.a. SQLITE_TRANSIENT).
    Some(unsafe {
        std::mem::transmute::<isize, unsafe extern "C" fn(*mut std::ffi::c_void)>(-1isize)
    })
}

/// Scalar SQLite function that returns the canonical zero Float as a hex string.
pub(crate) unsafe extern "C" fn float_zero_hex(
    context: *mut sqlite3_context,
    argc: c_int,
    _argv: *mut *mut sqlite3_value,
) {
    if argc != 0 {
        sqlite3_result_error(
            context,
            c"FLOAT_ZERO_HEX() does not take any arguments".as_ptr(),
            -1,
        );
        sqlite3_result_error_code(context, SQLITE_MISUSE);
        return;
    }

    let zero_hex = Float::default().as_hex();
    let zero_hex_ptr = zero_hex.as_ptr() as *const c_char;
    let zero_hex_len = zero_hex.len() as c_int;

    sqlite3_result_text(context, zero_hex_ptr, zero_hex_len, sqlite_transient());
}
