use anyhow::{Context, Result};

/// Parsed representation of a ContentId
///
/// ContentId can be in two formats:
/// - Legacy: `{blob_object_id}` - simple object ID
/// - Batched: `{blob_object_id}:{offset}:{length}` - object within a batched blob
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedContentId {
    /// Legacy format: simple blob object ID
    Legacy { blob_object_id: String },
    /// Batched format: object is a slice within a larger blob
    Batched {
        blob_object_id: String,
        offset: u64,
        length: u64,
    },
}

impl ParsedContentId {
    /// Parse a ContentId string into its components
    pub fn parse(content_id: &str) -> Result<Self> {
        let parts: Vec<&str> = content_id.split(':').collect();

        match parts.len() {
            1 => {
                // Legacy format: just the blob object ID
                Ok(ParsedContentId::Legacy {
                    blob_object_id: parts[0].to_string(),
                })
            }
            3 => {
                // Batched format: blob_object_id:offset:length
                let blob_object_id = parts[0].to_string();
                let offset = parts[1]
                    .parse::<u64>()
                    .with_context(|| format!("Invalid offset in ContentId: {}", parts[1]))?;
                let length = parts[2]
                    .parse::<u64>()
                    .with_context(|| format!("Invalid length in ContentId: {}", parts[2]))?;

                Ok(ParsedContentId::Batched {
                    blob_object_id,
                    offset,
                    length,
                })
            }
            _ => {
                anyhow::bail!("Invalid ContentId format: {}", content_id)
            }
        }
    }

    /// Create a batched ContentId
    pub fn batched(blob_object_id: String, offset: u64, length: u64) -> Self {
        ParsedContentId::Batched {
            blob_object_id,
            offset,
            length,
        }
    }

    /// Create a legacy ContentId
    pub fn legacy(blob_object_id: String) -> Self {
        ParsedContentId::Legacy { blob_object_id }
    }

    /// Get the blob object ID
    pub fn blob_object_id(&self) -> &str {
        match self {
            ParsedContentId::Legacy { blob_object_id } => blob_object_id,
            ParsedContentId::Batched {
                blob_object_id, ..
            } => blob_object_id,
        }
    }

    /// Check if this is a batched ContentId
    #[allow(dead_code)]
    pub fn is_batched(&self) -> bool {
        matches!(self, ParsedContentId::Batched { .. })
    }

    /// Encode back to ContentId string
    pub fn encode(&self) -> String {
        match self {
            ParsedContentId::Legacy { blob_object_id } => blob_object_id.clone(),
            ParsedContentId::Batched {
                blob_object_id,
                offset,
                length,
            } => format!("{}:{}:{}", blob_object_id, offset, length),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_legacy() {
        let content_id = "0xabc123";
        let parsed = ParsedContentId::parse(content_id).unwrap();

        assert_eq!(
            parsed,
            ParsedContentId::Legacy {
                blob_object_id: "0xabc123".to_string()
            }
        );
        assert!(!parsed.is_batched());
        assert_eq!(parsed.blob_object_id(), "0xabc123");
        assert_eq!(parsed.encode(), content_id);
    }

    #[test]
    fn test_parse_batched() {
        let content_id = "0xabc123:100:200";
        let parsed = ParsedContentId::parse(content_id).unwrap();

        assert_eq!(
            parsed,
            ParsedContentId::Batched {
                blob_object_id: "0xabc123".to_string(),
                offset: 100,
                length: 200,
            }
        );
        assert!(parsed.is_batched());
        assert_eq!(parsed.blob_object_id(), "0xabc123");
        assert_eq!(parsed.encode(), content_id);
    }

    #[test]
    fn test_create_batched() {
        let parsed = ParsedContentId::batched("0xdef456".to_string(), 50, 150);

        assert_eq!(parsed.encode(), "0xdef456:50:150");
        assert!(parsed.is_batched());
    }

    #[test]
    fn test_create_legacy() {
        let parsed = ParsedContentId::legacy("0x789xyz".to_string());

        assert_eq!(parsed.encode(), "0x789xyz");
        assert!(!parsed.is_batched());
    }

    #[test]
    fn test_parse_invalid_format() {
        let result = ParsedContentId::parse("0xabc:100");
        assert!(result.is_err());

        let result = ParsedContentId::parse("0xabc:100:200:extra");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_invalid_numbers() {
        let result = ParsedContentId::parse("0xabc:invalid:200");
        assert!(result.is_err());

        let result = ParsedContentId::parse("0xabc:100:invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_roundtrip() {
        let original_legacy = "0x1234567890abcdef";
        let parsed = ParsedContentId::parse(original_legacy).unwrap();
        assert_eq!(parsed.encode(), original_legacy);

        let original_batched = "0xfedcba0987654321:12345:67890";
        let parsed = ParsedContentId::parse(original_batched).unwrap();
        assert_eq!(parsed.encode(), original_batched);
    }
}
