//! Opaque stream tokens: random secrets a broadcaster presents to MediaMTX via
//! WHIP. Only their SHA-256 hashes are persisted in Redis.

use sha2::{Digest, Sha256};

/// Generates a fresh 32-byte random stream token, hex-encoded as 64 chars.
pub fn generate_stream_token() -> String {
    let bytes: [u8; 32] = rand::random();
    hex_encode(&bytes)
}

/// Returns the hex-encoded SHA-256 hash of `token`, suitable as a Redis lookup key.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let result = hasher.finalize();
    hex_encode(&result)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
