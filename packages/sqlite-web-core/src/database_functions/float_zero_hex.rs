use super::*;

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
        return;
    }

    let zero_hex = Float::default().as_hex();
    let zero_cstring = match CString::new(zero_hex) {
        Ok(s) => s,
        Err(e) => {
            let error_msg = format!("Failed to create zero hex string: {}\\0", e);
            sqlite3_result_error(context, error_msg.as_ptr() as *const c_char, -1);
            return;
        }
    };

    sqlite3_result_text(
        context,
        zero_cstring.as_ptr(),
        zero_cstring.as_bytes().len() as c_int,
        Some(std::mem::transmute::<
            isize,
            unsafe extern "C" fn(*mut std::ffi::c_void),
        >(-1isize)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn default_float_matches_parsed_zero_hex() {
        let default_hex = Float::default().as_hex();
        let parsed_zero_hex = Float::parse("0".to_string()).unwrap().as_hex();
        assert_eq!(default_hex, parsed_zero_hex);
    }
}
