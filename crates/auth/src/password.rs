//! Argon2id password hashing and verification.

use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};

/// Errors from hashing or verifying a password.
#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    /// The hasher failed to produce a hash from the input.
    #[error("failed to hash password")]
    Hash,
    /// The password did not match the stored hash, or the hash was malformed.
    #[error("invalid password")]
    Verify,
}

/// Hashes `password` with Argon2id using a fresh random salt.
///
/// # Errors
/// Returns [`PasswordError::Hash`] if hashing fails.
pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| PasswordError::Hash)?;
    Ok(hash.to_string())
}

/// Verifies `password` against a previously stored Argon2 `hash`.
///
/// # Errors
/// Returns [`PasswordError::Verify`] if the password does not match or the
/// hash string cannot be parsed.
pub fn verify_password(password: &str, hash: &str) -> Result<(), PasswordError> {
    let parsed = PasswordHash::new(hash).map_err(|_| PasswordError::Verify)?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| PasswordError::Verify)
}
