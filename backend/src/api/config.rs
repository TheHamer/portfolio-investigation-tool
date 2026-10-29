//! HTTP API settings: the optional `[api]` section of the configuration file.
//!
//! The whole section is optional, and so is every key in it; each setting has
//! its own default, applied independently of the others. An explicit invalid
//! value is an error, never replaced by the default.

use serde::Deserialize;
use thiserror::Error;

use crate::config::strict_uint;

/// The default is defined here and nowhere else: 10 MiB. It is used only when
/// the key (or the whole section) is absent.
fn default_max_upload_bytes() -> usize {
    10 * 1024 * 1024
}

/// The `[api]` section as written. Only `ApiConfig::from_raw` turns it into a
/// usable configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawApiConfig {
    #[serde(default = "default_max_upload_bytes", deserialize_with = "strict_uint")]
    max_upload_bytes: usize,
}

/// Used when the `[api]` section is absent altogether. Built from the same
/// per-field default functions as a present-but-empty section.
impl Default for RawApiConfig {
    fn default() -> Self {
        Self {
            max_upload_bytes: default_max_upload_bytes(),
        }
    }
}

/// Validated API settings. Built only from the configuration file (through
/// `Config::load`), so the values are always usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiConfig {
    max_upload_bytes: usize,
}

impl ApiConfig {
    pub(crate) fn from_raw(raw: RawApiConfig) -> Result<Self, ApiConfigError> {
        if raw.max_upload_bytes == 0 {
            return Err(ApiConfigError::ZeroMaxUploadBytes);
        }
        Ok(Self {
            max_upload_bytes: raw.max_upload_bytes,
        })
    }

    /// Maximum size, in bytes, of the whole HTTP request body of
    /// `POST /api/trades/import`: the multipart framing (boundaries and part
    /// headers) counts as well as the CSV file itself.
    pub fn max_upload_bytes(&self) -> usize {
        self.max_upload_bytes
    }
}

/// An unusable `[api]` setting. Reported at startup.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ApiConfigError {
    #[error("api.max_upload_bytes must be greater than 0")]
    ZeroMaxUploadBytes,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deserialises a section the way the loader does (via serde), then
    /// validates it.
    fn parse(json: serde_json::Value) -> Result<ApiConfig, String> {
        let raw: RawApiConfig = serde_json::from_value(json).map_err(|e| e.to_string())?;
        ApiConfig::from_raw(raw).map_err(|e| e.to_string())
    }

    #[test]
    fn missing_key_and_missing_section_use_the_default() {
        let empty_section = parse(serde_json::json!({})).unwrap();
        assert_eq!(empty_section.max_upload_bytes(), 10_485_760);

        let no_section = ApiConfig::from_raw(RawApiConfig::default()).unwrap();
        assert_eq!(no_section, empty_section);
    }

    #[test]
    fn explicit_value_is_kept() {
        let config = parse(serde_json::json!({ "max_upload_bytes": 2048 })).unwrap();
        assert_eq!(config.max_upload_bytes(), 2048);
        assert_eq!(
            parse(serde_json::json!({ "max_upload_bytes": 1 }))
                .unwrap()
                .max_upload_bytes(),
            1
        );
    }

    #[test]
    fn zero_is_rejected_not_defaulted() {
        assert_eq!(
            parse(serde_json::json!({ "max_upload_bytes": 0 })).unwrap_err(),
            ApiConfigError::ZeroMaxUploadBytes.to_string()
        );
    }

    #[test]
    fn invalid_types_and_values_are_rejected() {
        for bad in [
            serde_json::json!("lots"),
            serde_json::json!(-1),
            serde_json::json!(5.5),
            serde_json::json!(true),
            serde_json::json!(null),
            // Does not fit in a u64, so cannot be a usize either. Environment
            // variables arrive as strings like this one.
            serde_json::json!("18446744073709551616"),
        ] {
            assert!(
                parse(serde_json::json!({ "max_upload_bytes": bad })).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let error = parse(serde_json::json!({ "max_upload": 5 })).unwrap_err();
        assert!(error.contains("max_upload"), "{error}");
    }
}
