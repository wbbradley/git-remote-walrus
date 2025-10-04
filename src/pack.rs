//! Git pack format support for walrus
//!
//! This module handles Git's native pack format for push/fetch operations,
//! replacing the fast-import/fast-export approach to preserve GPG signatures
//! and maintain exact SHA-1 hashes.

pub mod objects;
pub mod receive;
pub mod send;

pub use receive::receive_pack;
pub use send::send_pack;
