//! Encrypts stored access tokens with Windows DPAPI. Mirrors `auto_dm/security.py`.
//!
//! Values are tagged so the Python and Rust versions read each other's tokens:
//! - `dpapi:` + base64url of the DPAPI blob on Windows
//! - `plain:` + base64url of the raw text elsewhere
//! - anything else is returned unchanged

use anyhow::{Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE;

pub fn protect_secret(value: &str) -> Result<String> {
    #[cfg(windows)]
    {
        let blob = dpapi::protect(value.as_bytes())?;
        Ok(format!("dpapi:{}", URL_SAFE.encode(blob)))
    }
    #[cfg(not(windows))]
    {
        Ok(format!("plain:{}", URL_SAFE.encode(value.as_bytes())))
    }
}

pub fn unprotect_secret(value: &str) -> Result<String> {
    if let Some(encoded) = value.strip_prefix("plain:") {
        return Ok(String::from_utf8(URL_SAFE.decode(encoded)?)?);
    }
    let Some(encoded) = value.strip_prefix("dpapi:") else {
        return Ok(value.to_string());
    };
    let encrypted = URL_SAFE.decode(encoded)?;
    #[cfg(windows)]
    {
        Ok(String::from_utf8(dpapi::unprotect(&encrypted)?)?)
    }
    #[cfg(not(windows))]
    {
        let _ = encrypted;
        bail!("DPAPI secret can only be opened on the Windows account that saved it")
    }
}

#[cfg(windows)]
mod dpapi {
    //! The only `unsafe` code in the app: calls into crypt32.dll.
    //!
    //! Memory rules for these calls:
    //! - The input blob only borrows our slice, so Rust still owns and frees it.
    //! - Windows allocates the output blob with LocalAlloc, so we must copy it into a Vec
    //!   and release the original with LocalFree exactly once.

    use super::*;
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    fn input_blob(data: &[u8]) -> Result<CRYPT_INTEGER_BLOB> {
        Ok(CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(data.len())?,
            // DPAPI does not write through this pointer; the cast only satisfies the C signature.
            pbData: data.as_ptr().cast_mut(),
        })
    }

    /// Copies the Windows-owned output into a Vec, then frees the Windows allocation.
    fn take_output(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY: on success Windows sets pbData to a valid buffer of cbData bytes.
        let bytes =
            unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec();
        // SAFETY: pbData came from LocalAlloc inside DPAPI and is freed only here.
        unsafe { LocalFree(blob.pbData.cast()) };
        bytes
    }

    pub fn protect(data: &[u8]) -> Result<Vec<u8>> {
        let input = input_blob(data)?;
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        // SAFETY: input points at live memory for the whole call and output is a valid out-param.
        let ok = unsafe {
            CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(anyhow!(std::io::Error::last_os_error()).context("CryptProtectData failed"));
        }
        Ok(take_output(output))
    }

    pub fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
        let input = input_blob(data)?;
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        // SAFETY: same contract as in protect.
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            bail!(
                "CryptUnprotectData failed: {}. The token was saved by another Windows account",
                std::io::Error::last_os_error()
            );
        }
        Ok(take_output(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_round_trip() {
        let protected = protect_secret("token-1").expect("protect");
        assert_ne!(protected, "token-1");
        assert_eq!(unprotect_secret(&protected).expect("unprotect"), "token-1");
    }

    #[test]
    fn untagged_and_plain_values_are_readable() {
        assert_eq!(unprotect_secret("raw-token").unwrap(), "raw-token");
        let plain = format!("plain:{}", URL_SAFE.encode("abc"));
        assert_eq!(unprotect_secret(&plain).unwrap(), "abc");
    }
}
