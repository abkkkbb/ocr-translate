use base64::{engine::general_purpose::STANDARD, Engine as _};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{LocalFree, HLOCAL},
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    },
};

const PREFIX: &str = "dpapi:";

pub fn is_protected(value: &str) -> bool {
    value.starts_with(PREFIX)
}

/// Encrypts with Windows DPAPI, so only the same Windows user on this machine can decrypt it.
pub fn protect(plain: &str) -> Result<String, String> {
    let mut input = plain.as_bytes().to_vec();
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &blob(&mut input),
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    }
    .map_err(|error| format!("加密密钥失败：{error}"))?;
    Ok(format!("{PREFIX}{}", STANDARD.encode(take_blob(output))))
}

pub fn unprotect(stored: &str) -> Result<String, String> {
    let encoded = stored.strip_prefix(PREFIX).ok_or("密钥不是加密格式。")?;
    let mut input = STANDARD
        .decode(encoded)
        .map_err(|error| format!("密钥格式无效：{error}"))?;
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &blob(&mut input),
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    }
    .map_err(|error| format!("解密密钥失败：{error}"))?;
    String::from_utf8(take_blob(output)).map_err(|_| "解密后的密钥不是有效文本。".into())
}

fn blob(bytes: &mut [u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_mut_ptr(),
    }
}

/// Copies a DPAPI output buffer and frees it with LocalFree, as the API requires.
fn take_blob(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if blob.pbData.is_null() {
        return Vec::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec();
    unsafe { LocalFree(Some(HLOCAL(blob.pbData.cast()))) };
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_without_storing_plaintext() {
        let stored = protect("example-secret").unwrap();
        assert!(is_protected(&stored));
        assert!(!stored.contains("example-secret"));
        assert_eq!(unprotect(&stored).unwrap(), "example-secret");
    }

    #[test]
    fn rejects_tampered_values() {
        assert!(unprotect("dpapi:AAAA").is_err());
        assert!(unprotect("plain-text").is_err());
    }
}
