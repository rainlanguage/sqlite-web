use super::*;

const FLOAT_IS_ZERO_ARG_ERROR_MESSAGE: &[u8] = b"FLOAT_IS_ZERO() requires exactly 1 argument\0";

fn float_is_zero_hex(input_hex: &str) -> Result<bool, String> {
    let trimmed = input_hex.trim();

    if trimmed.is_empty() {
        return Err("Empty string is not a valid hex number".to_string());
    }

    let float_val =
        Float::from_hex(trimmed).map_err(|e| format!("Failed to parse Float hex: {e}"))?;

    float_val
        .is_zero()
        .map_err(|e| format!("Failed to evaluate Float zero state: {e}"))
}

pub unsafe extern "C" fn float_is_zero(
    context: *mut sqlite3_context,
    argc: c_int,
    argv: *mut *mut sqlite3_value,
) {
    if argc != 1 {
        sqlite3_result_error(
            context,
            FLOAT_IS_ZERO_ARG_ERROR_MESSAGE.as_ptr() as *const c_char,
            -1,
        );
        return;
    }

    let value_type = sqlite3_value_type(*argv);
    let value_ptr = sqlite3_value_text(*argv);
    if value_ptr.is_null() {
        if value_type == SQLITE_NULL {
            sqlite3_result_null(context);
        } else {
            sqlite3_result_error_nomem(context);
        }
        return;
    }

    let value_str = CStr::from_ptr(value_ptr as *const c_char).to_string_lossy();

    match float_is_zero_hex(&value_str) {
        Ok(is_zero) => {
            sqlite3_result_int(context, if is_zero { 1 } else { 0 });
        }
        Err(e) => {
            let error_msg = format!("{e}\0");
            sqlite3_result_error(context, error_msg.as_ptr() as *const c_char, -1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    fn parse_decimal_to_hex(decimal: &str) -> String {
        Float::parse(decimal.to_string()).unwrap().as_hex()
    }

    #[wasm_bindgen_test]
    fn test_float_is_zero_hex_true_for_zero() {
        let zero_hex = parse_decimal_to_hex("0");
        assert!(float_is_zero_hex(&zero_hex).unwrap());
    }

    #[wasm_bindgen_test]
    fn test_float_is_zero_hex_false_for_non_zero() {
        let non_zero_hex = parse_decimal_to_hex("1.25");
        assert!(!float_is_zero_hex(&non_zero_hex).unwrap());
    }

    #[wasm_bindgen_test]
    fn test_float_is_zero_hex_handles_whitespace() {
        let zero_hex = parse_decimal_to_hex("0");
        let wrapped = format!("  {zero_hex}  ");
        assert!(float_is_zero_hex(&wrapped).unwrap());
    }

    #[wasm_bindgen_test]
    fn test_float_is_zero_hex_invalid_input() {
        assert!(float_is_zero_hex("").is_err());
        assert!(float_is_zero_hex("not_hex").is_err());
    }
}
