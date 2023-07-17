use crate::PruneMode;
use serde::{Deserialize, Deserializer};

/// Deserializes [`Option<PruneMode>`] and validates that the value is not less than the const
/// generic parameter `MIN_DISTANCE`.
///
/// 1. For [PruneMode::Full], it fails if `ALLOW_FULL == false`.
/// 2. For [PruneMode::Distance(distance)], it fails if `distance < MIN_DISTANCE`.
pub fn deserialize_opt_prune_mode_with_constraints<
    'de,
    const MIN_DISTANCE: u64,
    const ALLOW_FULL: bool,
    D: Deserializer<'de>,
>(
    deserializer: D,
) -> Result<Option<PruneMode>, D::Error> {
    let prune_mode = Option::<PruneMode>::deserialize(deserializer)?;

    match prune_mode {
        Some(PruneMode::Full) if !ALLOW_FULL => {
            Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Str("full"),
                // This message should have "expected" wording, so we say "to be supported"
                &"prune mode to be supported",
            ))
        }
        Some(PruneMode::Distance(distance)) if distance < MIN_DISTANCE => {
            Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Unsigned(distance),
                // This message should have "expected" wording, so we say "not less than"
                &format!("prune mode distance not less than {MIN_DISTANCE} blocks").as_str(),
            ))
        }
        _ => Ok(prune_mode),
    }
}

#[cfg(test)]
mod test {
    use crate::PruneMode;
    use assert_matches::assert_matches;
    use serde::Deserialize;

    #[test]
    fn deserialize_opt_prune_mode_with_constraints() {
        #[derive(Debug, Deserialize, PartialEq, Eq)]
        struct V(
            #[serde(
                deserialize_with = "super::deserialize_opt_prune_mode_with_constraints::<10, _>"
            )]
            Option<PruneMode>,
        );

        assert!(serde_json::from_str::<V>(r#"{"distance": 10}"#).is_ok());
        assert_matches!(
            serde_json::from_str::<V>(r#"{"distance": 9}"#),
            Err(err) if err.to_string() == "invalid value: integer `9`, expected prune mode distance not less than 10 blocks"
        );

        assert_matches!(
            serde_json::from_str::<V>(r#""full""#),
            Err(err) if err.to_string() == "invalid value: string \"full\", expected prune mode distance not less than 10 blocks"
        );
    }
}
