use reth_db::table::{Compress, Encode};

/// Returns bench vectors in the format: `Vec<(Key, EncodedKey, Value, CompressedValue)>`.
///
/// TBD, so for now only loads 3 default values.
#[allow(dead_code)]
fn load_vectors<T: reth_db::table::Table>() -> Vec<(T::Key, bytes::Bytes, T::Value, bytes::Bytes)>
where
    T: Default,
    T::Key: Default + Clone + for<'de> serde::Deserialize<'de>,
    T::Value: Default + Clone + for<'de> serde::Deserialize<'de>,
{
    let list: Vec<(T::Key, T::Value)> = bincode::deserialize_from(std::io::BufReader::new(
        std::fs::File::open(format!(
            "{}/../../../test-utils/test-vectors/{}",
            env!("CARGO_MANIFEST_DIR"),
            T::NAME
        ))
        .unwrap(),
    ))
    .unwrap();

    list.into_iter()
        .map(|(k, v)| {
            (
                k.clone(),
                bytes::Bytes::copy_from_slice(k.encode().as_ref()),
                v.clone(),
                bytes::Bytes::copy_from_slice(v.compress().as_ref()),
            )
        })
        .collect::<Vec<_>>()
}
