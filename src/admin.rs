//! Admin password hashing and HTTP Basic credential checks. Mirrors `auto_dm/admin.py`.

use sha2::{Digest, Sha256};

/// SHA-256 hex digest, compatible with hashes made by the Python version.
pub fn hash_admin_password(password: &str) -> String {
    hex::encode(Sha256::digest(password.as_bytes()))
}

/// Compares in time that depends only on the length, like Python `secrets.compare_digest`.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// An empty expected hash disables admin auth for local development.
pub fn valid_admin_credentials(
    username: &str,
    password: &str,
    expected_username: &str,
    expected_password_hash: &str,
) -> bool {
    if expected_password_hash.is_empty() {
        return true;
    }
    let username_ok = constant_time_eq(username.as_bytes(), expected_username.as_bytes());
    let password_hash = hash_admin_password(password);
    let password_ok = constant_time_eq(password_hash.as_bytes(), expected_password_hash.as_bytes());
    username_ok && password_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_credentials_check_hash() {
        let hash = hash_admin_password("secret");
        assert!(valid_admin_credentials("admin", "secret", "admin", &hash));
        assert!(!valid_admin_credentials("admin", "wrong", "admin", &hash));
        assert!(!valid_admin_credentials("other", "secret", "admin", &hash));
        assert!(valid_admin_credentials("anyone", "anything", "admin", ""));
    }

    #[test]
    fn hash_matches_python_sha256_hex() {
        assert_eq!(
            hash_admin_password("secret"),
            "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b"
        );
    }
}
