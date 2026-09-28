//! Protects secrets at rest: DPAPI on Windows (bound to the user account, no prompts),
//! plain bytes elsewhere, where the owner-only file mode is the protection.

use std::io;

/// Returns the scheme name stored alongside the sealed bytes.
pub fn seal(plain: &[u8]) -> io::Result<(&'static str, Vec<u8>)> {
    #[cfg(windows)]
    return dpapi::protect(plain).map(|sealed| ("dpapi", sealed));
    #[cfg(not(windows))]
    Ok(("plain", plain.to_vec()))
}

pub fn open(scheme: &str, sealed: &[u8]) -> io::Result<Vec<u8>> {
    match scheme {
        "plain" => Ok(sealed.to_vec()),
        #[cfg(windows)]
        "dpapi" => dpapi::unprotect(sealed),
        other => Err(io::Error::other(format!(
            "key sealed with `{other}`, which this machine cannot open"
        ))),
    }
}

#[cfg(windows)]
mod dpapi {
    use std::io;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    pub fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
        run(data, |input, output| unsafe {
            CryptProtectData(
                input,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                output,
            )
        })
    }

    pub fn unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
        run(data, |input, output| unsafe {
            CryptUnprotectData(
                input,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                output,
            )
        })
    }

    fn run(
        data: &[u8],
        call: impl FnOnce(*const CRYPT_INTEGER_BLOB, *mut CRYPT_INTEGER_BLOB) -> i32,
    ) -> io::Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(data.len()).map_err(io::Error::other)?,
            pbData: data.as_ptr().cast_mut(),
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: null_mut(),
        };
        if call(&input, &mut output) == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: on success the API hands us `cbData` bytes at `pbData`, ours to free.
        let bytes =
            unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
        unsafe { LocalFree(output.pbData.cast()) };
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let (scheme, sealed) = seal(b"4192083755612094").unwrap();
        assert_eq!(open(scheme, &sealed).unwrap(), b"4192083755612094");
        #[cfg(windows)]
        assert_ne!(sealed, b"4192083755612094");
    }

    #[test]
    fn unknown_scheme_is_an_error() {
        assert!(open("rot13", b"x").is_err());
    }
}
