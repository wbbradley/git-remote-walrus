//! Git pack format support for gitwal
//!
//! This module handles Git's native pack format for push/fetch operations,
//! replacing the fast-import/fast-export approach to preserve GPG signatures
//! and maintain exact SHA-1 hashes.

mod objects;
mod receive;
mod send;

pub use objects::{GitObject, ObjectId};
pub use receive::receive_pack;
pub use send::send_pack;
