use super::*;

#[test]
fn known_result_codes_keep_success_and_failure_semantics() {
    assert_eq!(check(EResult::k_EResultOK), Ok(()));
    for code in [
        EResult::k_EResultFail,
        EResult::k_EResultNoConnection,
        EResult::k_EResultInvalidParam,
        EResult::k_EResultLimitExceeded,
    ] {
        assert_eq!(send_error_result(-i64::from(code.0)), code);
        assert_eq!(check(code), Err(GnsError::Api(code)));
    }
}

#[test]
fn unknown_result_codes_are_preserved_as_errors() {
    // 4 is a gap in the headers; 130 is beyond the last declared result.
    for code in [EResult(4), EResult(130), EResult(0x7fff_ffff)] {
        assert_eq!(send_error_result(-i64::from(code.0)), code);
        assert_eq!(check(code), Err(GnsError::Api(code)));
    }
}

#[test]
#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn oversized_signed_send_errors_do_not_wrap() {
    // MSVC represents the native EResult as a signed 32-bit integer.
    for value in [-(i64::from(i32::MAX) + 1), -i64::from(u32::MAX)] {
        assert_eq!(send_error_result(value), EResult::k_EResultFail);
    }
}

#[test]
fn oversized_send_errors_fail_without_overflow_or_truncation() {
    for value in [
        -(i64::from(u32::MAX) + 1),
        -(i64::from(u32::MAX) + 2),
        i64::MIN + 1,
        i64::MIN,
    ] {
        assert_eq!(send_error_result(value), EResult::k_EResultFail);
    }
}
