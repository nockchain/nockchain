use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedFinalTip {
    pub height: u64,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedFinalTip {
    pub height: u64,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalTipValidation {
    pub expected: ExpectedFinalTip,
    pub observed: Option<ObservedFinalTip>,
    pub valid: bool,
    pub invalid_reason: Option<String>,
}

pub fn validate_final_tip(
    expected: ExpectedFinalTip,
    observed: Option<ObservedFinalTip>,
) -> FinalTipValidation {
    let invalid_reason = match &observed {
        Some(observed) if observed.height == expected.height && observed.hash == expected.hash => {
            None
        }
        Some(observed) => Some(format!(
            "final tip mismatch: expected {} {}, got {} {}",
            expected.height, expected.hash, observed.height, observed.hash
        )),
        None => Some("final tip unavailable after replay".to_string()),
    };

    FinalTipValidation {
        expected,
        observed,
        valid: invalid_reason.is_none(),
        invalid_reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_matching_tip() {
        let expected = ExpectedFinalTip {
            height: 7,
            hash: "abc".to_string(),
        };
        let observed = ObservedFinalTip {
            height: 7,
            hash: "abc".to_string(),
        };

        let result = validate_final_tip(expected, Some(observed));

        assert!(result.valid);
        assert_eq!(result.invalid_reason, None);
    }

    #[test]
    fn standardizes_mismatch_reason() {
        let result = validate_final_tip(
            ExpectedFinalTip {
                height: 7,
                hash: "abc".to_string(),
            },
            Some(ObservedFinalTip {
                height: 8,
                hash: "def".to_string(),
            }),
        );

        assert!(!result.valid);
        assert_eq!(
            result.invalid_reason.as_deref(),
            Some("final tip mismatch: expected 7 abc, got 8 def")
        );
    }

    #[test]
    fn standardizes_missing_tip_reason() {
        let result = validate_final_tip(
            ExpectedFinalTip {
                height: 7,
                hash: "abc".to_string(),
            },
            None,
        );

        assert!(!result.valid);
        assert_eq!(
            result.invalid_reason.as_deref(),
            Some("final tip unavailable after replay")
        );
    }
}
