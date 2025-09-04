use super::*;

// Helper to negate a Rain Float hex string by formatting to decimal, toggling sign,
// parsing back to Float, and returning the hex representation.
fn float_negate_hex_to_hex(input_hex: &str) -> Result<String, String> {
    let trimmed = input_hex.trim();

    // Parse the input hex into a Float
    let float_val =
        Float::from_hex(trimmed).map_err(|e| format!("Failed to parse Float hex: {e}"))?;

    // Convert to human-readable decimal
    let decimal = float_val
        .format()
        .map_err(|e| format!("Failed to format Float to decimal: {e}"))?;

    // Toggle sign on the decimal string
    let neg_decimal = if decimal.starts_with('-') {
        decimal.trim_start_matches('-').to_string()
    } else {
        format!("-{decimal}")
    };

    // Parse back to Float from decimal
    let neg_float = Float::parse(neg_decimal)
        .map_err(|e| format!("Failed to parse negated decimal to Float: {e}"))?;

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

    let value_str = CStr::from_ptr(value_ptr as *const c_char).to_string_lossy();

    match float_negate_hex_to_hex(&value_str) {
        Ok(result_hex) => {
            if let Ok(result_cstr) = CString::new(result_hex) {
                sqlite3_result_text(
                    context,
                    result_cstr.as_ptr(),
                    result_cstr.as_bytes().len() as c_int,
                    Some(std::mem::transmute::<
                        isize,
                        unsafe extern "C" fn(*mut std::ffi::c_void),
                    >(-1isize)), // SQLITE_TRANSIENT
                );
            } else {
                sqlite3_result_error(context, c"Failed to create result string".as_ptr(), -1);
            }
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
