use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;

use serde_json::json;

use crate::{ParseOptions, parse_bytes, parse_file, to_json};

const VERSION: &[u8] = b"0.1.0\0";

/// Return the ABI version as a static NUL-terminated string. Do not free it.
#[unsafe(no_mangle)]
pub extern "C" fn dscapture_version() -> *const c_char {
    VERSION.as_ptr().cast()
}

/// Parse a file and return UTF-8 JSON allocated by dscapture.
///
/// `options_json` may be NULL, in which case defaults are used. On success the
/// returned root object is the datasheet. On failure it is `{ "error": "..." }`.
/// Release the pointer with `dscapture_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn dscapture_parse_file_json(
    input_path: *const c_char,
    options_json: *const c_char,
) -> *mut c_char {
    ffi_json(|| {
        if input_path.is_null() {
            return Err("input_path is NULL".to_owned());
        }
        // SAFETY: The C API requires a readable NUL-terminated input string.
        let path = unsafe { CStr::from_ptr(input_path) }
            .to_str()
            .map_err(|error| format!("input_path is not UTF-8: {error}"))?;
        let options = ffi_options(options_json)?;
        let result = parse_file(path, &options).map_err(|error| error.to_string())?;
        to_json(&result, options.pretty_json).map_err(|error| error.to_string())
    })
}

/// Parse PDF bytes and return UTF-8 JSON allocated by dscapture.
///
/// `filename_hint` and `options_json` may be NULL. The caller retains ownership
/// of `data`; it only needs to remain valid for the duration of the call.
#[unsafe(no_mangle)]
pub extern "C" fn dscapture_parse_bytes_json(
    data: *const u8,
    length: usize,
    filename_hint: *const c_char,
    options_json: *const c_char,
) -> *mut c_char {
    ffi_json(|| {
        if data.is_null() && length != 0 {
            return Err("data is NULL while length is non-zero".to_owned());
        }
        // SAFETY: The C API requires `data` to point to at least `length` bytes.
        let bytes = if length == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(data, length) }
        };
        let hint = if filename_hint.is_null() {
            None
        } else {
            // SAFETY: The C API requires a readable NUL-terminated hint string.
            Some(
                unsafe { CStr::from_ptr(filename_hint) }
                    .to_str()
                    .map_err(|error| format!("filename_hint is not UTF-8: {error}"))?,
            )
        };
        let options = ffi_options(options_json)?;
        let result = parse_bytes(bytes, hint, &options).map_err(|error| error.to_string())?;
        to_json(&result, options.pretty_json).map_err(|error| error.to_string())
    })
}

/// Free a string returned by a dscapture C ABI function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dscapture_free_string(value: *mut c_char) {
    if !value.is_null() {
        // SAFETY: `value` must have been returned by this library and not freed before.
        drop(unsafe { CString::from_raw(value) });
    }
}

fn ffi_options(options_json: *const c_char) -> std::result::Result<ParseOptions, String> {
    if options_json.is_null() {
        return Ok(ParseOptions::default());
    }
    // SAFETY: The C API requires a readable NUL-terminated options string.
    let options = unsafe { CStr::from_ptr(options_json) }
        .to_str()
        .map_err(|error| format!("options_json is not UTF-8: {error}"))?;
    serde_json::from_str(options).map_err(|error| format!("invalid options JSON: {error}"))
}

fn ffi_json(operation: impl FnOnce() -> std::result::Result<String, String>) -> *mut c_char {
    let response = match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(json)) => json,
        Ok(Err(error)) => json!({ "error": error }).to_string(),
        Err(_) => json!({ "error": "unexpected parser panic" }).to_string(),
    };
    let response = response.replace('\0', "\\u0000");
    CString::new(response)
        .expect("NUL bytes were escaped")
        .into_raw()
}
