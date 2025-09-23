use sqlite_wasm_rs::export::*;
use std::ffi::{c_int, CStr, CString};
use std::os::raw::c_char;

// Import the individual function modules
mod bigint_sum;
mod float_is_zero;
mod float_negate;
mod float_sum;
mod float_zero_hex;

use bigint_sum::*;
use float_is_zero::*;
use float_negate::*;
use float_sum::*;
use float_zero_hex::*;

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

    // Register FLOAT_SUM aggregate function
    let float_sum_name = CString::new("FLOAT_SUM").unwrap();
    let ret = unsafe {
        sqlite3_create_function_v2(
            db,
            float_sum_name.as_ptr(),
            1, // 1 argument
            SQLITE_UTF8,
            std::ptr::null_mut(),
            None,                  // No xFunc for aggregate function
            Some(float_sum_step),  // xStep callback
            Some(float_sum_final), // xFinal callback
            None,                  // No destructor
        )
    };

    if ret != SQLITE_OK {
        return Err("Failed to register FLOAT_SUM function".to_string());
    }

    // Register FLOAT_ZERO_HEX scalar function
    let float_zero_hex_name = CString::new("FLOAT_ZERO_HEX").unwrap();
    let ret = unsafe {
        sqlite3_create_function_v2(
            db,
            float_zero_hex_name.as_ptr(),
            0, // 0 arguments
            SQLITE_UTF8 | SQLITE_DETERMINISTIC | SQLITE_INNOCUOUS,
            std::ptr::null_mut(),
            Some(float_zero_hex), // Scalar function
            None,
            None,
            None,
        )
    };

    if ret != SQLITE_OK {
        return Err("Failed to register FLOAT_ZERO_HEX function".to_string());
    }

    // Register FLOAT_NEGATE scalar function
    let float_negate_name = CString::new("FLOAT_NEGATE").unwrap();
    let ret = unsafe {
        sqlite3_create_function_v2(
            db,
            float_negate_name.as_ptr(),
            1, // 1 argument
            SQLITE_UTF8 | SQLITE_DETERMINISTIC | SQLITE_INNOCUOUS,
            std::ptr::null_mut(),
            Some(float_negate), // xFunc for scalar
            None,               // No xStep
            None,               // No xFinal
            None,               // No destructor
        )
    };

    if ret != SQLITE_OK {
        return Err("Failed to register FLOAT_NEGATE function".to_string());
    }

    // Register FLOAT_IS_ZERO scalar function
    let float_is_zero_name = CString::new("FLOAT_IS_ZERO").unwrap();
    let ret = unsafe {
        sqlite3_create_function_v2(
            db,
            float_is_zero_name.as_ptr(),
            1, // 1 argument
            SQLITE_UTF8 | SQLITE_DETERMINISTIC | SQLITE_INNOCUOUS,
            std::ptr::null_mut(),
            Some(float_is_zero), // xFunc for scalar
            None,                // No xStep
            None,                // No xFinal
            None,                // No destructor
        )
    };

    if ret != SQLITE_OK {
        return Err("Failed to register FLOAT_IS_ZERO function".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

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
