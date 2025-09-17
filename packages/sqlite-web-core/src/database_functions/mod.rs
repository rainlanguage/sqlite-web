use alloy::primitives::I256;
use sqlite_wasm_rs::export::*;
use std::ffi::{c_int, CStr, CString};
use std::os::raw::c_char;

// Import the individual function modules
mod bigint_sum;

use bigint_sum::*;

/// Register all custom functions with the SQLite database
pub fn register_custom_functions(db: *mut sqlite3) -> Result<(), String> {
    // Register BIGINT_SUM aggregate function
    let bigint_sum_name = CString::new("BIGINT_SUM").unwrap();
    let ret = unsafe {
        sqlite3_create_function_v2(
            db,
            bigint_sum_name.as_ptr(),
            1, // 1 argument
            SQLITE_UTF8,
            std::ptr::null_mut(),
            None,                   // No xFunc for aggregate function
            Some(bigint_sum_step),  // xStep callback
            Some(bigint_sum_final), // xFinal callback
            None,                   // No destructor
        )
    };

    if ret != SQLITE_OK {
        return Err("Failed to register BIGINT_SUM function".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn test_cstring_conversion() {
        let test_string = "test string with spaces and symbols!@#$%";
        let c_string_result = CString::new(test_string);
        assert!(
            c_string_result.is_ok(),
            "Should be able to convert to CString"
        );

        let c_string = c_string_result.unwrap();
        assert_eq!(c_string.to_string_lossy(), test_string);
    }

    #[wasm_bindgen_test]
    fn test_cstring_with_null_bytes() {
        let string_with_null = "test\0string";
        let c_string_result = CString::new(string_with_null);
        assert!(
            c_string_result.is_err(),
            "Strings with null bytes should fail CString conversion"
        );
    }
}
