use super::*;

// Helper to negate a Rain Float hex string while keeping full precision by
// operating on the binary representation directly.
fn float_negate_hex_to_hex(input_hex: &str) -> Result<String, String> {
    let trimmed = input_hex.trim();

    if trimmed.is_empty() {
        return Err("Empty string is not a valid hex number".to_string());
    }

    // Parse the input hex into a Float
    let float_val =
        Float::from_hex(trimmed).map_err(|e| format!("Failed to parse Float hex: {e}"))?;

    // Negate the float directly to avoid any formatting or precision loss.
    let neg_float = (-float_val).map_err(|e| format!("Failed to negate Float value: {e}"))?;

    // Return as hex string
    Ok(neg_float.as_hex())
}

// SQLite scalar function wrapper: FLOAT_NEGATE(hex_text)
pub unsafe extern "C" fn float_negate(
    context: *mut sqlite3_context,
    argc: c_int,
    argv: *mut *mut sqlite3_value,
) {
    if argc != 1 {
        sqlite3_result_error(
            context,
            c"FLOAT_NEGATE() requires exactly 1 argument".as_ptr(),
            -1,
        );
        return;
    }

    // Get the text value
    let value_ptr = sqlite3_value_text(*argv);
    if value_ptr.is_null() {
        sqlite3_result_null(context);
        return;
    }

    let value_cstr = CStr::from_ptr(value_ptr as *const c_char);
    let value_str = match value_cstr.to_str() {
        Ok(value_str) => value_str,
        Err(_) => {
            sqlite3_result_error(context, c"invalid UTF-8".as_ptr(), -1);
            return;
        }
    };

    match float_negate_hex_to_hex(value_str) {
        Ok(result_hex) => {
            if let Ok(result_cstr) = CString::new(result_hex) {
                sqlite3_result_text(
                    context,
                    result_cstr.as_ptr(),
                    result_cstr.as_bytes().len() as c_int,
                    SQLITE_TRANSIENT(),
                );
            } else {
                sqlite3_result_error(context, c"Failed to create result string".as_ptr(), -1);
            }
        }
        Err(e) => match CString::new(e) {
            Ok(error_msg) => {
                sqlite3_result_error(context, error_msg.as_ptr(), -1);
            }
            Err(_) => {
                sqlite3_result_error(
                    context,
                    c"Error message contained interior NUL".as_ptr(),
                    -1,
                );
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    #[wasm_bindgen_test]
    fn test_float_negate_hex_to_hex_pos_to_neg() {
        let pos_hex = Float::parse("1.5".to_string()).unwrap().as_hex();
        let expected_neg_hex = Float::parse("-1.5".to_string()).unwrap().as_hex();
        let out = float_negate_hex_to_hex(&pos_hex).unwrap();
        assert_eq!(out, expected_neg_hex);
    }

    #[wasm_bindgen_test]
    fn test_float_negate_hex_to_hex_neg_to_pos() {
        let neg_hex = Float::parse("-2.25".to_string()).unwrap().as_hex();
        let expected_pos_hex = Float::parse("2.25".to_string()).unwrap().as_hex();
        let out = float_negate_hex_to_hex(&neg_hex).unwrap();
        assert_eq!(out, expected_pos_hex);
    }

    #[wasm_bindgen_test]
    fn test_float_negate_hex_to_hex_zero() {
        let zero_hex = Float::parse("0".to_string()).unwrap().as_hex();
        let expected_zero_hex = Float::parse("0".to_string()).unwrap().as_hex();
        let out = float_negate_hex_to_hex(&zero_hex).unwrap();
        assert_eq!(out, expected_zero_hex);
    }

    #[wasm_bindgen_test]
    fn test_float_negate_hex_to_hex_high_precision() {
        let input = "300.123456789012345678";
        let in_hex = Float::parse(input.to_string()).unwrap().as_hex();
        let expected_hex = Float::parse(format!("-{input}")).unwrap().as_hex();
        let out = float_negate_hex_to_hex(&in_hex).unwrap();
        assert_eq!(out, expected_hex);
    }

    #[wasm_bindgen_test]
    fn test_float_negate_hex_to_hex_whitespace() {
        let in_hex = Float::parse("10".to_string()).unwrap().as_hex();
        let wrapped = format!("  {in_hex}  ");
        let expected_hex = Float::parse("-10".to_string()).unwrap().as_hex();
        let out = float_negate_hex_to_hex(&wrapped).unwrap();
        assert_eq!(out, expected_hex);
    }

    #[wasm_bindgen_test]
    fn test_float_negate_hex_to_hex_invalid() {
        assert!(float_negate_hex_to_hex("0XBADHEX").is_err());
        assert!(float_negate_hex_to_hex("").is_err());
        assert!(float_negate_hex_to_hex("not_hex").is_err());
    }
}
