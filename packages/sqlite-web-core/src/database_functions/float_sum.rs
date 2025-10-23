use super::*;
use rain_math_float::Float;

const FLOAT_SUM_ARG_ERROR_MESSAGE: &[u8] = b"FLOAT_SUM() requires exactly 1 argument\0";
const FLOAT_SUM_CONTEXT_ERROR_MESSAGE: &[u8] = b"Failed to allocate aggregate context\0";
const FLOAT_SUM_ZERO_HEX_ERROR_MESSAGE: &[u8] = b"Zero hex string contained interior NUL\0";

pub struct FloatSumContext {
    total: Float,
}

impl FloatSumContext {
    fn new() -> Self {
        Self {
            total: Float::default(),
        }
    }

    fn add_value(&mut self, value_str: &str) -> Result<(), String> {
        let trimmed = value_str.trim();

        if trimmed.is_empty() {
            return Err("Empty string is not a valid hex number".to_string());
        }

        let float_value = Float::from_hex(trimmed)
            .map_err(|e| format!("Failed to parse hex number '{}': {}", trimmed, e))?;

        self.total = (self.total + float_value).map_err(|e| {
            format!(
                "Float overflow when adding {} to running total: {}",
                trimmed, e
            )
        })?;

        Ok(())
    }

    fn get_total_as_hex(&self) -> Result<String, String> {
        // Return the hex representation of the accumulated Float
        Ok(self.total.as_hex())
    }
}

// Aggregate function step - called for each row
pub(crate) unsafe extern "C" fn float_sum_step(
    context: *mut sqlite3_context,
    argc: c_int,
    argv: *mut *mut sqlite3_value,
) {
    if argc != 1 {
        sqlite3_result_error(
            context,
            FLOAT_SUM_ARG_ERROR_MESSAGE.as_ptr() as *const c_char,
            -1,
        );
        return;
    }

    // Get the text value
    let value_ptr = sqlite3_value_text(*argv);
    if value_ptr.is_null() {
        return;
    }

    let value_str = CStr::from_ptr(value_ptr as *const c_char).to_string_lossy();

    // Get or create the aggregate context
    let aggregate_context =
        sqlite3_aggregate_context(context, std::mem::size_of::<FloatSumContext>() as c_int);
    if aggregate_context.is_null() {
        sqlite3_result_error(
            context,
            FLOAT_SUM_CONTEXT_ERROR_MESSAGE.as_ptr() as *const c_char,
            -1,
        );
        return;
    }

    // Cast to our context type
    let sum_context = aggregate_context as *mut FloatSumContext;

    // SQLite's sqlite3_aggregate_context allocates zeroed memory on first call
    // We can determine if this is the first call by checking if the memory is all zeros
    let bytes = std::slice::from_raw_parts(
        aggregate_context as *const u8,
        std::mem::size_of::<FloatSumContext>(),
    );
    let is_uninitialized = bytes.iter().all(|&b| b == 0);

    if is_uninitialized {
        std::ptr::write(sum_context, FloatSumContext::new());
    }

    // Add this value to the running total
    if let Err(e) = (*sum_context).add_value(&value_str) {
        let error_msg = format!("{}\0", e);
        sqlite3_result_error(context, error_msg.as_ptr() as *const c_char, -1)
    }
}

// Aggregate function final - called to return the final result
pub(crate) unsafe extern "C" fn float_sum_final(context: *mut sqlite3_context) {
    let aggregate_context = sqlite3_aggregate_context(context, 0);

    if aggregate_context.is_null() {
        // No rows were processed; surface the canonical zero hex string derived from Float::default().
        let zero_hex = Float::default().as_hex();
        match CString::new(zero_hex) {
            Ok(zero_result) => {
                sqlite3_result_text(
                    context,
                    zero_result.as_ptr(),
                    zero_result.as_bytes().len() as c_int,
                    Some(std::mem::transmute::<
                        isize,
                        unsafe extern "C" fn(*mut std::ffi::c_void),
                    >(-1isize)),
                );
            }
            Err(_) => {
                sqlite3_result_error(
                    context,
                    FLOAT_SUM_ZERO_HEX_ERROR_MESSAGE.as_ptr() as *const c_char,
                    -1,
                );
            }
        }
        return;
    }

    let sum_context = aggregate_context as *mut FloatSumContext;
    let result_str = match (*sum_context).get_total_as_hex() {
        Ok(s) => s,
        Err(e) => {
            let error_msg = format!("{}\0", e);
            sqlite3_result_error(context, error_msg.as_ptr() as *const c_char, -1);
            std::ptr::drop_in_place(sum_context);
            return;
        }
    };

    let result_cstring = match CString::new(result_str) {
        Ok(s) => s,
        Err(e) => {
            let error_msg = format!("Failed to create result string: {}\0", e);
            sqlite3_result_error(context, error_msg.as_ptr() as *const c_char, -1);
            std::ptr::drop_in_place(sum_context);
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

    std::ptr::drop_in_place(sum_context);
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    #[wasm_bindgen_test]
    fn test_float_sum_context_new() {
        let context = FloatSumContext::new();
        let zero = Float::parse("0".to_string()).unwrap();
        assert_eq!(context.total.format().unwrap(), zero.format().unwrap());
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "0");
    }

    #[wasm_bindgen_test]
    fn test_float_sum_context_add_hex_values() {
        let mut context = FloatSumContext::new();

        assert!(context
            .add_value(Float::parse("0.1".to_string()).unwrap().as_hex().as_str())
            .is_ok()); // 0.1
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "0.1");

        assert!(context
            .add_value(Float::parse("0.5".to_string()).unwrap().as_hex().as_str())
            .is_ok()); // 0.5
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "0.6"); // 0.1 + 0.5 = 0.6
    }

    #[wasm_bindgen_test]
    fn test_float_sum_context_add_hex_without_prefix() {
        let mut context = FloatSumContext::new();

        let one_point_five = Float::parse("1.5".to_string()).unwrap().as_hex();
        let one_point_five_no_prefix = one_point_five.trim_start_matches("0x").to_string();

        assert!(context.add_value(&one_point_five_no_prefix).is_ok()); // 1.5
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "1.5");

        let two_point_two_five = Float::parse("2.25".to_string()).unwrap().as_hex();
        let two_point_two_five_no_prefix = two_point_two_five.trim_start_matches("0x").to_string();

        assert!(context.add_value(&two_point_two_five_no_prefix).is_ok()); // 2.25
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "3.75"); // 1.5 + 2.25 = 3.75
    }

    #[wasm_bindgen_test]
    fn test_float_sum_context_add_uppercase_hex() {
        let mut context = FloatSumContext::new();

        let upper_case_bad = Float::parse("-12345.6789".to_string())
            .unwrap()
            .as_hex()
            .replacen("0x", "0X", 1);
        assert!(context.add_value(&upper_case_bad).is_err()); // Should fail - uppercase 0X not supported

        let another_upper = Float::parse("1024.125".to_string())
            .unwrap()
            .as_hex()
            .replacen("0x", "0X", 1);
        assert!(context.add_value(&another_upper).is_err()); // Should fail - uppercase 0X not supported
    }

    #[wasm_bindgen_test]
    fn test_float_sum_context_invalid_input() {
        let mut context = FloatSumContext::new();

        assert!(context.add_value("not_hex").is_err());
        assert!(context.add_value("0xGHI").is_err());
        assert!(context.add_value("").is_err());
        assert!(context.add_value("   ").is_err());
    }

    #[wasm_bindgen_test]
    fn test_float_sum_context_large_hex_values() {
        let mut context = FloatSumContext::new();

        let large_hex1 = Float::parse("100.25".to_string()).unwrap();
        let large_hex2 = Float::parse("123.456".to_string()).unwrap(); // 123.456

        assert!(context.add_value(&large_hex1.as_hex()).is_ok());
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "100.25");

        assert!(context.add_value(&large_hex2.as_hex()).is_ok());
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "223.706"); // 100.25 + 123.456 = 223.706
    }

    #[wasm_bindgen_test]
    fn test_float_sum_context_zero_values() {
        let mut context = FloatSumContext::new();

        assert!(context
            .add_value(Float::default().as_hex().as_str())
            .is_ok()); // 0
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "0");

        assert!(context
            .add_value(Float::parse("0.1".to_string()).unwrap().as_hex().as_str())
            .is_ok()); // 0.1
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "0.1"); // 0 + 0.1 = 0.1
    }

    #[wasm_bindgen_test]
    fn test_float_sum_context_mixed_case_hex() {
        let mut context = FloatSumContext::new();

        assert!(context
            .add_value(Float::parse("1.5".to_string()).unwrap().as_hex().as_str())
            .is_ok()); // 1.5
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "1.5");

        assert!(context
            .add_value(Float::parse("2.25".to_string()).unwrap().as_hex().as_str())
            .is_ok()); // 2.25
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "3.75"); // 1.5 + 2.25 = 3.75
    }

    #[wasm_bindgen_test]
    fn test_float_sum_context_edge_case_hex() {
        let mut context = FloatSumContext::new();

        assert!(context.add_value("0x0").is_err()); // Should fail - too short
        assert!(context.add_value("0xF").is_err()); // Should fail - too short
    }

    #[wasm_bindgen_test]
    fn test_float_sum_context_whitespace_handling() {
        let mut context = FloatSumContext::new();

        assert!(context
            .add_value(&format!(
                " {} ",
                Float::parse("10".to_string()).unwrap().as_hex()
            ))
            .is_ok()); // 10
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "10");

        assert!(context
            .add_value(&format!(
                "\t{}\n",
                Float::parse("20".to_string()).unwrap().as_hex()
            ))
            .is_ok()); // 20
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "30"); // 10 + 20 = 30
    }

    #[wasm_bindgen_test]
    fn test_float_sum_context_high_precision_decimals() {
        let mut context = FloatSumContext::new();

        let high_precision_hex_values = vec![
            Float::parse("300.123456789012345678".to_string()).unwrap(),
            Float::parse("300.987654321098765432".to_string()).unwrap(),
            Float::parse("300.555555555555555555".to_string()).unwrap(),
            Float::parse("300.777777777777777777".to_string()).unwrap(),
            Float::parse("300.999999999999999999".to_string()).unwrap(),
        ];

        for hex_val in high_precision_hex_values {
            assert!(context.add_value(&hex_val.as_hex()).is_ok());
        }

        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "1503.444444443444444441"); // Sum of all 5 high-precision values
    }

    #[wasm_bindgen_test]
    fn test_float_sum_context_mixed_precision_values() {
        let mut context = FloatSumContext::new();

        assert!(context
            .add_value(
                Float::parse("1.123456789012345678".to_string())
                    .unwrap()
                    .as_hex()
                    .as_str()
            )
            .is_ok()); // 1.123456789012345678
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "1.123456789012345678");

        assert!(context
            .add_value(
                Float::parse("2.987654321098765432".to_string())
                    .unwrap()
                    .as_hex()
                    .as_str()
            )
            .is_ok()); // 2.987654321098765432
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "4.11111111011111111"); // 1.123456789012345678 + 2.987654321098765432

        assert!(context
            .add_value(Float::parse("100".to_string()).unwrap().as_hex().as_str())
            .is_ok()); // 100
        let result_hex = context.get_total_as_hex().unwrap();
        let result_decimal = Float::from_hex(&result_hex).unwrap().format().unwrap();
        assert_eq!(result_decimal, "104.11111111011111111"); // 4.11111111011111111 + 100
    }
}
