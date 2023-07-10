use crate::PruneMode;
use serde::{Deserialize, Deserializer};

/// Deserializes [`Option<PruneMode>`] and validates that the value contained in
/// [PruneMode::Distance] (if any) is not less than the const generic parameter `MIN_DISTANCE`.
pub fn deserialize_opt_prune_mode_with_min_distance<
    'de,
    const MIN_DISTANCE: u64,
    D: Deserializer<'de>,
>(
    deserializer: D,
) -> Result<Option<PruneMode>, D::Error> {
    let prune_mode = Option::<PruneMode>::deserialize(deserializer)?;

    match prune_mode {
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
    fn deserialize_opt_prune_mode_with_min_distance() {
        #[derive(Debug, Deserialize, PartialEq, Eq)]
        struct V(
            #[serde(
                deserialize_with = "super::deserialize_opt_prune_mode_with_min_distance::<10, _>"
            )]
            Option<PruneMode>,
        );

        assert!(serde_json::from_str::<V>(r#"{"distance": 10}"#).is_ok());
        assert_matches!(
            serde_json::from_str::<V>(r#"{"distance": 9}"#),
            Err(err) if err.to_string() == "invalid value: integer `9`, expected prune mode distance not less than 10 blocks"
        );
    }
}
