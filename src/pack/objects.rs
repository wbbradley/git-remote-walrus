//! Git object handling using gitoxide

use anyhow::{Context, Result};
use gix_object::Kind;
use sha1::{Digest, Sha1};
use std::io::Read;

/// Git object SHA-1 identifier (40 hex characters)
pub type ObjectId = String;

/// Represents a Git object with its content
#[derive(Debug, Clone)]
pub struct GitObject {
    pub id: ObjectId,
    pub kind: Kind,
    pub data: Vec<u8>,
}

impl GitObject {
    /// Create a GitObject from raw object data (without header)
    pub fn from_raw(kind: Kind, data: Vec<u8>) -> Result<Self> {
        let id = compute_object_id(kind, &data)?;
        Ok(Self { id, kind, data })
    }

    /// Parse a loose object file (with header: "type size\0data")
    pub fn from_loose_format(content: &[u8]) -> Result<Self> {
        // Parse the header manually
        let null_pos = content
            .iter()
            .position(|&b| b == 0)
            .context("No null terminator in object header")?;

        let header = std::str::from_utf8(&content[..null_pos])
            .context("Invalid UTF-8 in object header")?;

        let parts: Vec<&str> = header.split_whitespace().collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid object header format: {}", header);
        }

        let kind = match parts[0] {
            "commit" => Kind::Commit,
            "tree" => Kind::Tree,
            "blob" => Kind::Blob,
            "tag" => Kind::Tag,
            _ => anyhow::bail!("Unknown object type: {}", parts[0]),
        };

        let data = content[null_pos + 1..].to_vec();
        let id = compute_object_id(kind, &data)?;

        Ok(Self { id, kind, data })
    }

    /// Serialize to loose object format (with header)
    pub fn to_loose_format(&self) -> Vec<u8> {
        let kind_str = match self.kind {
            Kind::Commit => "commit",
            Kind::Tree => "tree",
            Kind::Blob => "blob",
            Kind::Tag => "tag",
        };
        let header = format!("{} {}\0", kind_str, self.data.len());
        let mut result = header.into_bytes();
        result.extend_from_slice(&self.data);
        result
    }

    /// Get the object data without header
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

/// Compute Git SHA-1 object ID from object type and data
fn compute_object_id(kind: Kind, data: &[u8]) -> Result<ObjectId> {
    let kind_str = match kind {
        Kind::Commit => "commit",
        Kind::Tree => "tree",
        Kind::Blob => "blob",
        Kind::Tag => "tag",
    };
    let header = format!("{} {}\0", kind_str, data.len());
    let mut hasher = Sha1::new();
    hasher.update(header.as_bytes());
    hasher.update(data);
    let hash = hasher.finalize();
    Ok(hex::encode(hash))
}

/// Read a loose object from filesystem path
pub fn read_loose_object(path: &std::path::Path) -> Result<GitObject> {
    // Loose objects are zlib compressed
    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open object file: {}", path.display()))?;
    let mut decoder = flate2::read::ZlibDecoder::new(file);
    let mut content = Vec::new();
    decoder
        .read_to_end(&mut content)
        .context("Failed to decompress object")?;

    GitObject::from_loose_format(&content)
}

/// Write a loose object to filesystem path (creates intermediate directories)
pub fn write_loose_object(obj: &GitObject, base_path: &std::path::Path) -> Result<std::path::PathBuf> {
    // Loose objects stored as .git/objects/ab/cdef123...
    let (dir, file) = obj.id.split_at(2);
    let obj_dir = base_path.join(dir);
    std::fs::create_dir_all(&obj_dir)
        .with_context(|| format!("Failed to create object directory: {}", obj_dir.display()))?;

    let obj_path = obj_dir.join(file);

    // Compress and write
    let content = obj.to_loose_format();
    let file = std::fs::File::create(&obj_path)
        .with_context(|| format!("Failed to create object file: {}", obj_path.display()))?;
    let mut encoder = flate2::write::ZlibEncoder::new(file, flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, &content)
        .context("Failed to write compressed object")?;
    encoder.finish().context("Failed to finish compression")?;

    Ok(obj_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_object_id() {
        // Known blob: "test\n" -> SHA-1: 9daeafb9864cf43055ae93beb0afd6c7d144bfa4
        let data = b"test\n";
        let id = compute_object_id(Kind::Blob, data).unwrap();
        assert_eq!(id, "9daeafb9864cf43055ae93beb0afd6c7d144bfa4");
    }

    #[test]
    fn test_loose_format_roundtrip() {
        let obj = GitObject::from_raw(Kind::Blob, b"hello world\n".to_vec()).unwrap();
        let loose = obj.to_loose_format();
        let parsed = GitObject::from_loose_format(&loose).unwrap();

        assert_eq!(obj.id, parsed.id);
        assert_eq!(obj.data, parsed.data);
    }
}
