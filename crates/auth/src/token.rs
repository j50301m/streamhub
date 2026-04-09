use rand::Rng;
use sha2::{Digest, Sha256};

/// Generate a random stream token (32 bytes, hex-encoded = 64 chars).
pub fn generate_stream_token() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = rng.r#gen();
    hex_encode(&bytes)
}

/// SHA-256 hash a token string, returning hex-encoded hash.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let result = hasher.finalize();
    hex_encode(&result)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
