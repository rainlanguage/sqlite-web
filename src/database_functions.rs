use sqlite_wasm_rs::export::*;
use std::ffi::{c_int, CStr, CString};
use std::os::raw::c_char;
use rain_math_float::Float;
use std::ops::Add;

// Custom function - alternates case (1st lowercase, 2nd uppercase, 3rd lowercase, etc.)
unsafe extern "C" fn alternating_case_function(
    context: *mut sqlite3_context,
    argc: c_int,
    argv: *mut *mut sqlite3_value,
) {
    if argc != 1 {
        sqlite3_result_error(
            context,
            "alternating_case() requires exactly 1 argument\0".as_ptr() as *const c_char,
            -1,
        );
        return;
    }

    let input = sqlite3_value_text(*argv);
    if input.is_null() {
        sqlite3_result_null(context);
        return;
    }

    let input_str = CStr::from_ptr(input as *const c_char).to_string_lossy();
    
    // Apply alternating case: 1st char lowercase, 2nd uppercase, 3rd lowercase, etc.
    let alternating_str: String = input_str
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if i % 2 == 0 {
                // Even index (0, 2, 4, ...) - lowercase
                c.to_lowercase().collect::<String>()
            } else {
                // Odd index (1, 3, 5, ...) - uppercase
                c.to_uppercase().collect::<String>()
            }
        })
        .collect();
    
    let result_cstring = CString::new(alternating_str).unwrap();

    sqlite3_result_text(
        context,
        result_cstring.as_ptr(),
        result_cstring.as_bytes().len() as c_int,
        Some(std::mem::transmute(-1isize)), // SQLITE_TRANSIENT
    );
}

// Custom function using rain-math-float library - sums two Float values
unsafe extern "C" fn rain_math_process(
    context: *mut sqlite3_context,
    argc: c_int,
    argv: *mut *mut sqlite3_value,
) {
    if argc != 2 {
        sqlite3_result_error(
            context,
            "rain_math_process() requires exactly 2 arguments\0".as_ptr() as *const c_char,
            -1,
        );
        return;
    }

    // Get first parameter
    let input1 = sqlite3_value_text(*argv);
    if input1.is_null() {
        sqlite3_result_null(context);
        return;
    }
    let input1_str = CStr::from_ptr(input1 as *const c_char).to_string_lossy();

    // Get second parameter
    let input2 = sqlite3_value_text(*argv.offset(1));
    if input2.is_null() {
        sqlite3_result_null(context);
        return;
    }
    let input2_str = CStr::from_ptr(input2 as *const c_char).to_string_lossy();

    // Convert strings to Float using rain-math-float
    let float1 = match Float::parse(input1_str.to_string()) {
        Ok(f) => f,
        Err(e) => {
            let error_msg = format!("Failed to parse first argument as Float: {}\0", e);
            sqlite3_result_error(
                context,
                error_msg.as_ptr() as *const c_char,
                -1,
            );
            return;
        }
    };

    let float2 = match Float::parse(input2_str.to_string()) {
        Ok(f) => f,
        Err(e) => {
            let error_msg = format!("Failed to parse second argument as Float: {}\0", e);
            sqlite3_result_error(
                context,
                error_msg.as_ptr() as *const c_char,
                -1,
            );
            return;
        }
    };

    // Sum the two Float values
    let result = match float1.add(float2) {
        Ok(r) => r,
        Err(e) => {
            let error_msg = format!("Failed to add Float values: {}\0", e);
            sqlite3_result_error(
                context,
                error_msg.as_ptr() as *const c_char,
                -1,
            );
            return;
        }
    };

    // Convert result back to string
    let result_str = match result.format() {
        Ok(s) => s,
        Err(e) => {
            let error_msg = format!("Failed to format result as string: {}\0", e);
            sqlite3_result_error(
                context,
                error_msg.as_ptr() as *const c_char,
                -1,
            );
            return;
        }
    };
    let result_cstring = CString::new(result_str).unwrap();

    sqlite3_result_text(
        context,
        result_cstring.as_ptr(),
        result_cstring.as_bytes().len() as c_int,
        Some(std::mem::transmute(-1isize)), // SQLITE_TRANSIENT
    );
}

/// Register all custom functions with the SQLite database
pub fn register_custom_functions(db: *mut sqlite3) -> Result<(), String> {
    // Register ALTERNATING_CASE function
    let func_name = CString::new("ALTERNATING_CASE").unwrap();
    let ret = unsafe {
        sqlite3_create_function_v2(
            db,
            func_name.as_ptr(),
            1, // 1 argument
            SQLITE_UTF8,
            std::ptr::null_mut(),
            Some(alternating_case_function),
            None, // No xStep for scalar function
            None, // No xFinal for scalar function
            None, // No destructor
        )
    };

    if ret != SQLITE_OK {
        return Err("Failed to register ALTERNATING_CASE function".to_string());
    }

    // Register rain_math_process function
    let func_name = CString::new("RAIN_MATH_PROCESS").unwrap();
    let ret = unsafe {
        sqlite3_create_function_v2(
            db,
            func_name.as_ptr(),
            2, // 2 arguments
            SQLITE_UTF8,
            std::ptr::null_mut(),
            Some(rain_math_process),
            None, // No xStep for scalar function
            None, // No xFinal for scalar function
            None, // No destructor
        )
    };

    if ret != SQLITE_OK {
        return Err("Failed to register RAIN_MATH_PROCESS function".to_string());
    }

    Ok(())
}