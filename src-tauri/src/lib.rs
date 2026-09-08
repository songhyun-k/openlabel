pub mod ble;
pub mod ipc;
pub mod job;
pub mod label;
pub mod m110;
pub mod operation;
pub mod raster;
pub mod settings;

use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct LocalizedMessage {
    pub key: String,
    pub params: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<Box<LocalizedMessage>>,
}
impl LocalizedMessage {
    fn english(&self) -> String {
        static ENGLISH: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
        let catalog = ENGLISH.get_or_init(|| {
            serde_json::from_str(include_str!("../../src/locales/en.json"))
                .expect("valid English message catalog")
        });
        let template = catalog.get(&self.key).expect("known message key");
        let cause = self.cause.as_ref().map(|cause| cause.english());
        // Substitute in the template once; braces inside external data stay literal.
        template
            .split_inclusive('}')
            .map(|part| {
                if let Some((prefix, name)) = part
                    .strip_suffix('}')
                    .and_then(|part| part.rsplit_once('{'))
                {
                    let value = if name == "cause" {
                        cause.as_ref().or_else(|| self.params.get(name))
                    } else {
                        self.params.get(name)
                    };
                    if let Some(value) = value {
                        return format!("{prefix}{value}");
                    }
                }
                part.to_owned()
            })
            .collect()
    }
}
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Error {
    pub code: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<LocalizedMessage>,
}
impl Error {
    pub fn localized(code: &str, key: &str, params: &[(&str, String)]) -> Self {
        let message = LocalizedMessage {
            key: key.into(),
            params: params
                .iter()
                .map(|(name, value)| ((*name).into(), value.clone()))
                .collect(),
            cause: None,
        };
        Self {
            code: code.into(),
            detail: message.english(),
            message: Some(message),
        }
    }
    pub fn with_context(self, key: &str) -> Self {
        let message = LocalizedMessage {
            key: key.into(),
            params: if self.message.is_none() {
                BTreeMap::from([("cause".into(), self.detail)])
            } else {
                BTreeMap::new()
            },
            cause: self.message.map(Box::new),
        };
        Self {
            code: self.code,
            detail: message.english(),
            message: Some(message),
        }
    }
    pub fn new(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
            message: None,
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
pub fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod localization_tests {
    use super::*;

    #[test]
    fn error_metadata_roundtrips_and_old_errors_keep_their_cause() {
        let old = r#"{"code":"bluetooth_error","detail":"external {cause}: 프린터"}"#;
        let old: Error = serde_json::from_str(old).unwrap();
        assert!(old.message.is_none());
        assert!(serde_json::to_value(&old).unwrap().get("message").is_none());
        let contextual = old.with_context("err.observerRestart");
        assert!(
            contextual
                .detail
                .starts_with("external {cause}: 프린터 Restart")
        );
        assert_eq!(contextual.code, "bluetooth_error");
        let error = Error::localized(
            "invalid_label",
            "err.svgAttribute",
            &[("name", "external {name}".into())],
        )
        .with_context("err.observerRestart");
        assert!(
            error
                .detail
                .starts_with("Attribute not allowed: external {name} Restart")
        );
        let wire = serde_json::to_string(&error).unwrap();
        let restored: Error = serde_json::from_str(&wire).unwrap();
        assert_eq!(restored.detail, error.detail);
        assert_eq!(restored.code, error.code);
        assert_eq!(
            restored
                .message
                .as_ref()
                .unwrap()
                .cause
                .as_ref()
                .unwrap()
                .params["name"],
            "external {name}"
        );
        assert_eq!(serde_json::to_string(&restored).unwrap(), wire);
    }
}
