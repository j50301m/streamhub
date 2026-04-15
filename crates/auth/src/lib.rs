//! Authentication primitives: JWT access/refresh tokens, password hashing, and
//! opaque stream tokens used by broadcasters when pushing to MediaMTX.
#![warn(missing_docs)]

pub mod jwt;
pub mod password;
pub mod token;
