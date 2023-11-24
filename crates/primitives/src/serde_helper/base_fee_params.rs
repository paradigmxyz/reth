use crate::{chain::BaseFeeParamsWrapper, BaseFeeParams, Hardfork};
use serde::{
    de::{self, SeqAccess, Visitor},
    ser::SerializeSeq,
    Deserializer, Serializer,
};

/// Deserialize the [BaseFeeParamsWrapper]
pub fn deserialize_base_fee_params<'de, D>(
    deserializer: D,
) -> Result<BaseFeeParamsWrapper, D::Error>
where
    D: Deserializer<'de>,
{
    struct BaseFeeParamsWrapperVisitor;

    impl<'de> Visitor<'de> for BaseFeeParamsWrapperVisitor {
        type Value = BaseFeeParamsWrapper;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("enum BaseFeeParamsWrapper")
        }

        fn visit_seq<V>(self, mut seq: V) -> Result<BaseFeeParamsWrapper, V::Error>
        where
            V: SeqAccess<'de>,
        {
            let first = seq.next_element::<BaseFeeParams>()?;
            if let Some(base_fee_params) = first {
                // Found a Constant variant
                Ok(BaseFeeParamsWrapper::Constant(base_fee_params))
            } else {
                // Expecting a Variable variant, which should be a sequence
                let variable_params = seq
                    .next_element::<Vec<(Hardfork, BaseFeeParams)>>()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                Ok(BaseFeeParamsWrapper::Variable(variable_params))
            }
        }
    }

    deserializer.deserialize_any(BaseFeeParamsWrapperVisitor)
}

/// Serialize the [BaseFeeParamsWrapper]
pub fn serialize_base_fee_params<S>(
    base_fee_params: &BaseFeeParamsWrapper,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match base_fee_params {
        BaseFeeParamsWrapper::Constant(base_fee_params) => serializer.serialize_newtype_variant(
            "BaseFeeParamsWrapper",
            0,
            "Constant",
            base_fee_params,
        ),
        BaseFeeParamsWrapper::Variable(variable_params) => {
            let mut seq = serializer.serialize_seq(Some(variable_params.len()))?;
            for (hardfork, params) in variable_params {
                seq.serialize_element(&(hardfork, params))?;
            }
            seq.end()
        }
    }
}
