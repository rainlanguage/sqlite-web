use super::*;

// Context structure for BIGINT_SUM aggregate function
pub struct BigIntSumContext {
    total: U256,
}

impl BigIntSumContext {
    fn new() -> Self {
        Self { total: U256::ZERO }
    }

    fn add_value(&mut self, value_str: &str) -> Result<(), String> {
        // Handle empty string as an error
        if value_str.trim().is_empty() {
            return Err("Empty string is not a valid number".to_string());
        }

        if let Some(num_str) = value_str.strip_prefix('-') {
            if num_str.is_empty() {
                return Err("Invalid negative number format".to_string());
            }
            let num = U256::from_str(num_str)
                .map_err(|e| format!("Failed to parse negative number '{}': {}", value_str, e))?;
            self.total = self.total.saturating_sub(num);
        } else {
            let num = U256::from_str(value_str)
                .map_err(|e| format!("Failed to parse positive number '{}': {}", value_str, e))?;
            self.total = self.total.saturating_add(num);
        }
        Ok(())
    }

    fn get_result(&self) -> String {
        self.total.to_string()
    }
}

// Aggregate function step - called for each row
pub unsafe extern "C" fn bigint_sum_step(
    context: *mut sqlite3_context,
    argc: c_int,
    argv: *mut *mut sqlite3_value,
) {
    if argc != 1 {
        sqlite3_result_error(
            context,
            c"BIGINT_SUM() requires exactly 1 argument".as_ptr(),
            -1,
        );
        return;
    }

    // Get the text value
    let value_ptr = sqlite3_value_text(*argv);
    if value_ptr.is_null() {
        sqlite3_result_error(context, c"BIGINT_SUM() received NULL value".as_ptr(), -1);
        return;
    }

    let value_str = CStr::from_ptr(value_ptr as *const c_char).to_string_lossy();

    // Get or create the aggregate context
    let aggregate_context =
        sqlite3_aggregate_context(context, std::mem::size_of::<BigIntSumContext>() as c_int);
    if aggregate_context.is_null() {
        sqlite3_result_error(
            context,
            c"Failed to allocate aggregate context".as_ptr(),
            -1,
        );
        return;
    }

    // Cast to our context type
    let sum_context = aggregate_context as *mut BigIntSumContext;

    // SQLite's sqlite3_aggregate_context allocates zeroed memory on first call
    // We can determine if this is the first call by checking if the memory is all zeros
    let mut is_uninitialized = true;
    let bytes = std::slice::from_raw_parts(
        aggregate_context as *const u8,
        std::mem::size_of::<BigIntSumContext>(),
    );
    for &byte in bytes {
        if byte != 0 {
            is_uninitialized = false;
            break;
        }
    }

    if is_uninitialized {
        std::ptr::write(sum_context, BigIntSumContext::new());
    }

    // Add this value to the running total
    if let Err(e) = (*sum_context).add_value(&value_str) {
        let error_msg = format!("{}\0", e);
        sqlite3_result_error(context, error_msg.as_ptr() as *const c_char, -1);
    }
}

// Aggregate function final - called to return the final result
pub unsafe extern "C" fn bigint_sum_final(context: *mut sqlite3_context) {
    let aggregate_context =
        sqlite3_aggregate_context(context, std::mem::size_of::<BigIntSumContext>() as c_int);

    if aggregate_context.is_null() {
        // No values were processed, return 0
        let zero_result = CString::new("0").unwrap();
        sqlite3_result_text(
            context,
            zero_result.as_ptr(),
            zero_result.as_bytes().len() as c_int,
            Some(std::mem::transmute::<
                isize,
                unsafe extern "C" fn(*mut std::ffi::c_void),
            >(-1isize)), // SQLITE_TRANSIENT
        );
        return;
    }

    let sum_context = aggregate_context as *mut BigIntSumContext;
    let result_str = (*sum_context).get_result();

    let result_cstring = match CString::new(result_str) {
        Ok(s) => s,
        Err(e) => {
            let error_msg = format!("Failed to create result string: {}\0", e);
            sqlite3_result_error(context, error_msg.as_ptr() as *const c_char, -1);
            return;
        }
    };

    sqlite3_result_text(
        context,
        result_cstring.as_ptr(),
        result_cstring.as_bytes().len() as c_int,
        Some(std::mem::transmute::<
            isize,
            unsafe extern "C" fn(*mut std::ffi::c_void),
        >(-1isize)), // SQLITE_TRANSIENT
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    // Tests for BigIntSumContext
    #[wasm_bindgen_test]
    fn test_bigint_sum_context_new() {
        let context = BigIntSumContext::new();
        assert_eq!(context.total, U256::ZERO);
        assert_eq!(context.get_result(), "0");
    }

    #[wasm_bindgen_test]
    fn test_bigint_sum_context_add_positive() {
        let mut context = BigIntSumContext::new();
        assert!(context.add_value("123").is_ok());
        assert_eq!(context.get_result(), "123");

        assert!(context.add_value("456").is_ok());
        assert_eq!(context.get_result(), "579");
    }

    #[wasm_bindgen_test]
    fn test_bigint_sum_context_add_negative() {
        let mut context = BigIntSumContext::new();
        assert!(context.add_value("100").is_ok());
        assert_eq!(context.get_result(), "100");

        assert!(context.add_value("-30").is_ok());
        assert_eq!(context.get_result(), "70");
    }

    #[wasm_bindgen_test]
    fn test_bigint_sum_context_mixed_values() {
        let mut context = BigIntSumContext::new();
        assert!(context.add_value("1000").is_ok());
        assert!(context.add_value("-200").is_ok());
        assert!(context.add_value("50").is_ok());
        assert!(context.add_value("-100").is_ok());
        assert_eq!(context.get_result(), "750");
    }

    #[wasm_bindgen_test]
    fn test_bigint_sum_context_large_numbers() {
        let mut context = BigIntSumContext::new();
        let large_num = "123456789012345678901234567890";
        assert!(context.add_value(large_num).is_ok());
        assert_eq!(context.get_result(), large_num);

        let another_large = "987654321098765432109876543210";
        assert!(context.add_value(another_large).is_ok());

        let expected = U256::from_str(large_num).unwrap() + U256::from_str(another_large).unwrap();
        assert_eq!(context.get_result(), expected.to_string());
    }

    #[wasm_bindgen_test]
    fn test_bigint_sum_context_saturating_sub() {
        let mut context = BigIntSumContext::new();
        assert!(context.add_value("50").is_ok());

        // This should saturate to 0 since we can't go negative with U256
        let very_large_negative = format!("-{}", U256::MAX);
        assert!(context.add_value(&very_large_negative).is_ok());
        assert_eq!(context.get_result(), "0");
    }

    #[wasm_bindgen_test]
    fn test_bigint_sum_context_invalid_input() {
        let mut context = BigIntSumContext::new();
        assert!(context.add_value("not_a_number").is_err());
        assert!(context.add_value("-not_a_number").is_err());
        assert!(context.add_value("").is_err());
        assert!(context.add_value("   ").is_err()); // whitespace only
        assert!(context.add_value("123abc").is_err());
        assert!(context.add_value("-").is_err()); // just a dash
    }

    #[wasm_bindgen_test]
    fn test_bigint_sum_context_edge_cases() {
        let mut context = BigIntSumContext::new();

        // Test zero values
        assert!(context.add_value("0").is_ok());
        assert_eq!(context.get_result(), "0");

        assert!(context.add_value("-0").is_ok());
        assert_eq!(context.get_result(), "0");

        // Test leading zeros
        assert!(context.add_value("000123").is_ok());
        assert_eq!(context.get_result(), "123");

        assert!(context.add_value("-000456").is_ok());
        assert!(context.get_result().parse::<i32>().unwrap() < 123);
    }

    #[wasm_bindgen_test]
    fn test_bigint_sum_context_hex_input() {
        let mut context = BigIntSumContext::new();
        // U256::from_str supports hex format
        assert!(context.add_value("0x10").is_ok()); // 16 in decimal
        assert_eq!(context.get_result(), "16");

        assert!(context.add_value("0xFF").is_ok()); // 255 in decimal
        assert_eq!(context.get_result(), "271"); // 16 + 255
    }

    #[wasm_bindgen_test]
    fn test_bigint_sum_context_max_u256() {
        let mut context = BigIntSumContext::new();
        let max_str = U256::MAX.to_string();
        assert!(context.add_value(&max_str).is_ok());
        assert_eq!(context.get_result(), max_str);

        // Adding more should saturate
        assert!(context.add_value("1").is_ok());
        assert_eq!(context.get_result(), U256::MAX.to_string());
    }
}
