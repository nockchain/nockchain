//! CORS policy enforced at the public gRPC API edge.
//!
//! The public gRPC services are served with gRPC-Web support so browsers can
//! call them. Browser cross-origin requests are gated by an explicit origin
//! allowlist supplied via the `NOCKCHAIN_API_CORS_ALLOWED_ORIGINS` environment
//! variable (comma- or whitespace-separated). When the variable is empty or
//! unset, no browser origin is allowed and every cross-origin browser request
//! is rejected by the CORS layer.
//!
//! Native gRPC (tonic) clients send no `Origin` header, so the CORS layer is a
//! no-op for them and they continue to work regardless of the allowlist.
//!
//! Only the origin dimension is restricted; methods, request headers, and
//! exposed response headers stay permissive so gRPC-Web continues to function
//! for an allowed origin. The variable is read once at server startup.

use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tracing::{info, warn};

/// Environment variable holding the browser CORS origin allowlist
/// (comma/whitespace separated). Empty or unset => no browser origins allowed.
pub const CORS_ORIGINS_ENV_VAR: &str = "NOCKCHAIN_API_CORS_ALLOWED_ORIGINS";

/// Parse an allowlist of origins from the raw env-var value. Blank tokens and
/// the wildcard `*` (which would reopen unrestricted CORS) are logged and
/// skipped.
fn parse_origins(raw: &str) -> Vec<String> {
    let mut origins = Vec::new();
    for token in raw.split([',', ' ', '\t', '\n', '\r']) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if token == "*" {
            warn!(
                "Ignoring wildcard '*' in {CORS_ORIGINS_ENV_VAR}; \
                 list explicit origins instead"
            );
            continue;
        }
        origins.push(token.to_string());
    }
    origins
}

/// Whether `origin` (the raw bytes of a request `Origin` header) is in the
/// configured allowlist.
fn origin_allowed(allowed: &[String], origin: &[u8]) -> bool {
    allowed.iter().any(|a| a.as_bytes() == origin)
}

/// Build the public-API CORS layer from `NOCKCHAIN_API_CORS_ALLOWED_ORIGINS`.
///
/// Methods, request headers, and exposed response headers remain permissive so
/// gRPC-Web keeps working for an allowed origin — only the set of allowed
/// browser origins is restricted. An empty allowlist rejects every browser
/// cross-origin request.
pub fn cors_layer_from_env() -> CorsLayer {
    let origins = std::env::var(CORS_ORIGINS_ENV_VAR)
        .map(|raw| parse_origins(&raw))
        .unwrap_or_default();

    if origins.is_empty() {
        info!(
            "API CORS allowlist is empty; all browser cross-origin requests will be rejected \
             (set {CORS_ORIGINS_ENV_VAR} to allow specific origins)"
        );
    } else {
        info!("API CORS allowlist active with {} origin(s)", origins.len());
    }

    let allow_origin =
        AllowOrigin::predicate(move |origin, _parts| origin_allowed(&origins, origin.as_bytes()));

    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers(Any)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_when_unset() {
        assert!(parse_origins("").is_empty());
    }

    #[test]
    fn parses_comma_and_whitespace_separated_origins() {
        let origins = parse_origins("https://a.example , https://b.example");
        assert_eq!(
            origins,
            vec!["https://a.example".to_string(), "https://b.example".to_string()]
        );
    }

    #[test]
    fn skips_wildcard_and_blanks() {
        let origins = parse_origins("*, ,https://ok.example");
        assert_eq!(origins, vec!["https://ok.example".to_string()]);
    }

    #[test]
    fn origin_match_is_exact() {
        let allowed = parse_origins("https://ok.example");
        assert!(origin_allowed(&allowed, b"https://ok.example"));
        assert!(!origin_allowed(&allowed, b"https://evil.example"));
        // No origins configured => nothing is allowed.
        assert!(!origin_allowed(&[], b"https://ok.example"));
    }

    #[test]
    fn builds_layer_without_panicking() {
        // The predicate path never panics (unlike AllowOrigin::list on a
        // wildcard); just confirm construction works for empty + non-empty.
        let _empty = cors_layer_from_env();
        let allowed = parse_origins("https://ok.example");
        let _allow = AllowOrigin::predicate(move |origin, _parts| {
            origin_allowed(&allowed, origin.as_bytes())
        });
    }
}
