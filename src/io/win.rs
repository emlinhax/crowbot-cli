//! What Windows calls need in common.

/// `s` as a NUL-terminated UTF-16 string, what Win32 calls take.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
