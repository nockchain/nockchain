//! Helpers for resolving start heights for speed-of-light runs.

use thiserror::Error;

use super::types::SolHeight;

#[derive(Debug, Error)]
pub enum StartHeightError {
    #[error("checkpoint height required but unavailable")]
    MissingCheckpointHeight,
}

/// Resolve the start height based on explicit and checkpoint-derived heights.
pub fn resolve_start_height(
    explicit_start: Option<SolHeight>,
    checkpoint_height: Option<SolHeight>,
) -> Result<SolHeight, StartHeightError> {
    if let Some(height) = explicit_start {
        return Ok(height);
    }
    if let Some(height) = checkpoint_height {
        return Ok(height.saturating_add(1));
    }
    Ok(SolHeight::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_start_height_explicit_overrides() {
        let resolved =
            resolve_start_height(Some(SolHeight(5)), Some(SolHeight(10))).expect("should resolve");
        assert_eq!(resolved, SolHeight(5));
    }

    #[test]
    fn test_resolve_start_height_from_checkpoint() {
        let resolved = resolve_start_height(None, Some(SolHeight(7))).expect("should resolve");
        assert_eq!(resolved, SolHeight(8));
    }

    #[test]
    fn test_resolve_start_height_defaults_to_zero() {
        let resolved = resolve_start_height(None, None).expect("should resolve");
        assert_eq!(resolved, SolHeight::ZERO);
    }
}
