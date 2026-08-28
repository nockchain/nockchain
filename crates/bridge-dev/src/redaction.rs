use std::collections::{BTreeMap, BTreeSet};

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use serde_json::Value as JsonValue;
use thiserror::Error;
use toml::Value as TomlValue;

const REDACTED: &str = "[REDACTED]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretValue {
    pub category: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct SecretRedactor {
    variants: Vec<(String, String)>,
    categories: BTreeSet<String>,
}

impl SecretRedactor {
    pub fn new(secrets: impl IntoIterator<Item = SecretValue>) -> Result<Self, RedactionError> {
        let mut variants = BTreeMap::new();
        let mut categories = BTreeSet::new();
        for secret in secrets {
            if secret.category.trim().is_empty() || secret.value.len() < 4 {
                return Err(RedactionError::InvalidSecretDeclaration);
            }
            categories.insert(secret.category.clone());
            for variant in encoded_variants(&secret.value) {
                if variant.len() >= 4 {
                    variants.insert(variant, secret.category.clone());
                }
            }
        }
        let mut variants = variants.into_iter().collect::<Vec<_>>();
        variants.sort_by(|left, right| right.0.len().cmp(&left.0.len()));
        Ok(Self {
            variants,
            categories,
        })
    }

    pub fn categories(&self) -> Vec<String> {
        self.categories.iter().cloned().collect()
    }

    pub fn redact_text(&self, input: &str) -> String {
        let mut output = input.to_owned();
        for (secret, category) in &self.variants {
            let marker = format!("[REDACTED:{category}]");
            output = output.replace(secret, &marker);
        }
        scrub_url_credentials(&output)
    }

    pub fn redact_json(&self, value: &mut JsonValue) {
        match value {
            JsonValue::Object(object) => {
                for (key, child) in object {
                    if let Some(category) = sensitive_field_category(key) {
                        *child = JsonValue::String(format!("[REDACTED:{category}]"));
                    } else {
                        self.redact_json(child);
                    }
                }
            }
            JsonValue::Array(values) => {
                for child in values {
                    self.redact_json(child);
                }
            }
            JsonValue::String(value) => *value = self.redact_text(value),
            _ => {}
        }
    }

    pub fn redact_toml(&self, value: &mut TomlValue) {
        match value {
            TomlValue::Table(table) => {
                for (key, child) in table {
                    if let Some(category) = sensitive_field_category(key) {
                        *child = TomlValue::String(format!("[REDACTED:{category}]"));
                    } else {
                        self.redact_toml(child);
                    }
                }
            }
            TomlValue::Array(values) => {
                for child in values {
                    self.redact_toml(child);
                }
            }
            TomlValue::String(value) => *value = self.redact_text(value),
            _ => {}
        }
    }

    pub fn assert_safe(&self, output: &[u8]) -> Result<(), RedactionError> {
        let text = String::from_utf8_lossy(output);
        for (secret, category) in &self.variants {
            if text.contains(secret) {
                return Err(RedactionError::SecretLeak {
                    category: category.clone(),
                });
            }
        }
        Ok(())
    }
}

fn encoded_variants(value: &str) -> BTreeSet<String> {
    let bytes = value.as_bytes();
    BTreeSet::from([
        value.to_owned(),
        percent_encode(bytes, true),
        percent_encode(bytes, false),
        STANDARD.encode(bytes),
        STANDARD.encode(bytes).trim_end_matches('=').to_owned(),
        URL_SAFE_NO_PAD.encode(bytes),
    ])
}

fn percent_encode(bytes: &[u8], uppercase: bool) -> String {
    const UPPER: &[u8; 16] = b"0123456789ABCDEF";
    const LOWER: &[u8; 16] = b"0123456789abcdef";
    let digits = if uppercase { UPPER } else { LOWER };
    let mut encoded = String::with_capacity(bytes.len() * 3);
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(digits[(byte >> 4) as usize]));
            encoded.push(char::from(digits[(byte & 0x0f) as usize]));
        }
    }
    encoded
}

fn sensitive_field_category(key: &str) -> Option<&'static str> {
    let key = key.to_ascii_lowercase();
    if key.contains("private_key") || key.ends_with("_key_pem") {
        Some("private_key")
    } else if key.contains("password") || key.contains("credential") {
        Some("credential")
    } else if key.contains("access_key") || key.contains("secret_key") {
        Some("object_store_credential")
    } else if key.contains("authorization")
        || key == "token"
        || key.ends_with("_auth_token")
        || key.ends_with("_api_token")
        || key.ends_with("_bearer_token")
        || key.ends_with("_session_token")
        || key.ends_with("_object_store_token")
    {
        Some("token")
    } else if (key.contains("rpc") || key.contains("url"))
        && (key.contains("secret") || key.contains("authenticated"))
    {
        Some("rpc_credential")
    } else {
        None
    }
}

fn scrub_url_credentials(input: &str) -> String {
    input
        .split_inclusive(char::is_whitespace)
        .map(|token| {
            let whitespace = token
                .chars()
                .rev()
                .take_while(|character| character.is_whitespace())
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>();
            let core = token.trim_end_matches(char::is_whitespace);
            let trimmed = core.trim_matches(|character: char| {
                matches!(character, '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';')
            });
            let prefix_len = core.find(trimmed).unwrap_or(0);
            let suffix_start = prefix_len + trimmed.len();
            let sanitized = sanitize_url(trimmed).unwrap_or_else(|| trimmed.to_owned());
            format!(
                "{}{}{}{}",
                &core[..prefix_len],
                sanitized,
                &core[suffix_start..],
                whitespace
            )
        })
        .collect()
}

fn sanitize_url(input: &str) -> Option<String> {
    if !input.starts_with("http://") && !input.starts_with("https://") {
        return None;
    }
    let mut url = reqwest::Url::parse(input).ok()?;
    let has_sensitive_parts = !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some();
    if !has_sensitive_parts {
        return Some(input.to_owned());
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    Some(format!("{}?{REDACTED}", url.as_str().trim_end_matches('/')))
}

#[derive(Debug, Error)]
pub enum RedactionError {
    #[error("secret declarations require nonempty categories and values of at least four bytes")]
    InvalidSecretDeclaration,
    #[error("safe evidence contains a {category} secret variant")]
    SecretLeak { category: String },
}
