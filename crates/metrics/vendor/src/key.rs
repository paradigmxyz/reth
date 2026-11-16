use crate::{cow::Cow, IntoLabels, Label, SharedString};
use rapidhash::v3::{rapidhash_v3_nano_inline, RapidSecrets};
use std::{
    borrow::Borrow,
    cmp, fmt,
    hash::{Hash, Hasher},
    slice::Iter,
};

const NO_LABELS: [Label; 0] = [];

/// Name component of a key.
#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct KeyName(SharedString);

impl KeyName {
    /// Creates a `KeyName` from a static string.
    pub const fn from_const_str(name: &'static str) -> Self {
        KeyName(SharedString::const_str(name))
    }

    /// Gets a reference to the string used for this name.
    pub const fn as_str(&self) -> &str {
        self.0.as_const_str()
    }

    /// Consumes this [`KeyName`], returning the inner [`SharedString`].
    pub fn into_inner(self) -> SharedString {
        self.0
    }

    /// Returns a version of this name that is cheap to clone for long-lived retention.
    ///
    /// *NOTE:* This will allocate if this `KeyName` was created from a non-`'static`
    /// string, however, the returned `KeyName` will not require allocation when cloned
    /// or `to_retained` is invoked again.
    pub fn to_retained(&self) -> Self {
        KeyName(self.0.to_retained())
    }
}

impl<T> From<T> for KeyName
where
    T: Into<SharedString>,
{
    fn from(name: T) -> Self {
        KeyName(name.into())
    }
}

impl Borrow<str> for KeyName {
    fn borrow(&self) -> &str {
        self.0.borrow()
    }
}

impl From<KeyName> for std::borrow::Cow<'static, str> {
    fn from(name: KeyName) -> Self {
        name.0.into()
    }
}

/// A metric identifier.
///
/// A key represents both the name and labels of a metric.
///
/// # Safety
/// Clippy will report any usage of `Key` as the key of a map/set as "mutable key type", meaning
/// that it believes that there is interior mutability present which could lead to a key being
/// hashed different over time.  That behavior could lead to unexpected behavior, as standard
/// maps/sets depend on keys having stable hashes over time, related to times when they must be
/// recomputed as the internal storage is resized and items are moved around.
///
/// In this case, the `Hash` implementation of `Key` does _not_ depend on the fields which Clippy
/// considers mutable (the atomics) and so it is actually safe against differing hashes being
/// generated.  We personally allow this Clippy lint in places where we store the key, such as
/// helper types in the `metrics-util` crate, and you may need to do the same if you're using it in
/// such a way as well.
#[derive(Debug, Clone)]
pub struct Key {
    name: KeyName,
    labels: Cow<'static, [Label]>,
    hash: u64,
}

impl Key {
    /// Creates a [`Key`] from a name.
    pub fn from_name<N>(name: N) -> Self
    where
        N: Into<KeyName>,
    {
        let name = name.into();
        let labels = Cow::from_owned(Vec::new());

        Self::builder(name, labels)
    }

    /// Creates a [`Key`] from a name and set of labels.
    pub fn from_parts<N, L>(name: N, labels: L) -> Self
    where
        N: Into<KeyName>,
        L: IntoLabels,
    {
        let name = name.into();
        let labels = Cow::from_owned(labels.into_labels());

        Self::builder(name, labels)
    }

    /// Creates a [`Key`] from a non-static name and a static set of labels.
    pub fn from_static_labels<N>(name: N, labels: &'static [Label]) -> Self
    where
        N: Into<KeyName>,
    {
        Self::builder(name.into(), Cow::const_slice(labels))
    }

    /// Creates a [`Key`] from a static name.
    ///
    /// This function is `const`, so it can be used in a static context.
    pub const fn from_static_name(name: &'static str) -> Self {
        Self::from_static_parts(name, &NO_LABELS)
    }

    /// Creates a [`Key`] from a static name and static set of labels.
    ///
    /// This function is `const`, so it can be used in a static context.
    pub const fn from_static_parts(name: &'static str, labels: &'static [Label]) -> Self {
        Self::builder(KeyName::from_const_str(name), Cow::const_slice(labels))
    }

    const fn builder(name: KeyName, labels: Cow<'static, [Label]>) -> Self {
        let hash = generate_key_hash(&name, &labels);
        Self { name, labels, hash }
    }

    /// Name of this key.
    pub fn name(&self) -> &str {
        self.name.0.as_ref()
    }

    /// Name of this key as a [`KeyName`]
    pub fn name_shared(&self) -> KeyName {
        self.name.clone()
    }

    /// Labels of this key, if they exist.
    pub fn labels(&self) -> Iter<'_, Label> {
        self.labels.iter()
    }

    /// Consumes this [`Key`], returning the name parts and any labels.
    pub fn into_parts(self) -> (KeyName, Vec<Label>) {
        (self.name, self.labels.into_owned())
    }

    /// Returns a version of this key that is cheap to clone for long-lived retention.
    ///
    /// *NOTE:* This will allocate if this `Key` was created from non-`'static`
    /// parts, however, the returned `Key` will not require allocation when cloned
    /// or `to_retained` is invoked again.
    pub fn to_retained(&self) -> Self {
        Self { name: self.name.to_retained(), labels: self.labels.to_retained(), hash: self.hash }
    }

    /// Clones this [`Key`], and expands the existing set of labels.
    pub fn with_extra_labels(&self, extra_labels: Vec<Label>) -> Self {
        if extra_labels.is_empty() {
            return self.clone();
        }

        let name = self.name.clone();
        let mut labels = self.labels.clone().into_owned();
        labels.extend(extra_labels);

        Self::builder(name, labels.into())
    }

    /// Gets the hash value for this key.
    #[inline]
    pub const fn get_hash(&self) -> u64 {
        self.hash
    }
}

/// Generate a hash value for a `Key`.
#[inline]
const fn generate_key_hash(name: &KeyName, labels: &Cow<'static, [Label]>) -> u64 {
    // Here we explicitly use a static seed. We believe HashDoS should not be a concern here, as
    // the primary use case is static metric keys coming from the application code itself. Users
    // could allow untrusted input to form a metric key, but it's not a use case we are designing
    // for. If this ever changes, using const_random!(u64) could be a clean solution to randomize
    // the seed at _build_ time.
    // github issue: https://github.com/metrics-rs/metrics/pull/651#issuecomment-3744517372
    // const_random: https://crates.io/crates/const-random
    const SECRETS: RapidSecrets = RapidSecrets::seed_cpp(0);

    // The name uses an independent seed to avoid simple substitution errors (where a user swaps a
    // label with the key name for example). We use `seed_cpp` to ensure the secrets arrays are the
    // same const array so the compiler can optimise the loading of secrets as the same immediates.
    let mut hash = hash_bytes(name.0.as_const_str().as_bytes(), &SECRETS);

    // Label order should _not_ change the resulting hash. The use of addition here is a hack,
    // this is both faster than sorting the labels and feasible in `const`. We use an ADD chain
    // as opposed to an XOR chain to avoid duplicate labels cancelling out.
    let mut i = 0;
    let labels = labels.as_const_slice();
    while i < labels.len() {
        let label_hash = hash_label(&labels[i]);
        hash = hash.wrapping_add(label_hash);
        i += 1;
    }

    hash
}

/// The underlying hash function used for keys and labels.
#[inline(always)]
const fn hash_bytes(slice: &[u8], secrets: &RapidSecrets) -> u64 {
    // We expect keys and labels to typically be <48 bytes, and so we choose rapidhash nano. We
    // skip the AVALANCHE mix step for a slightly lower-quality hash, as speed is preferably over
    // the highest hash "quality".
    rapidhash_v3_nano_inline::<false, false>(slice, secrets)
}

/// Hash a label, taking care to hash keys and values with independent seeds.
#[inline(always)]
const fn hash_label(Label(key, value): &Label) -> u64 {
    // hash the key and value with independent seeds to avoid substitution errors, but use
    // seed_cpp to ensure the secrets arrays are the same const array
    const KEY: RapidSecrets = RapidSecrets::seed_cpp(1);
    const VALUE: RapidSecrets = RapidSecrets::seed_cpp(2);

    let key = hash_bytes(key.as_const_str().as_bytes(), &KEY);
    let value = hash_bytes(value.as_const_str().as_bytes(), &VALUE);

    key ^ value
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        if self.hash != other.hash {
            return false;
        }
        if self.name != other.name {
            return false;
        }
        if self.labels.len() != other.labels.len() {
            return false;
        }
        match self.labels.len() {
            0 => true,
            1 => self.labels[0] == other.labels[0],
            2 => {
                if self.labels[0] == other.labels[0] {
                    self.labels[1] == other.labels[1]
                } else if self.labels[0] == other.labels[1] {
                    self.labels[1] == other.labels[0]
                } else {
                    false
                }
            }
            n if n < 8 => {
                let mut labels_sort_map: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
                labels_sort_map[..n].sort_by_key(|i| self.labels[*i as usize].key());
                let mut his_labels_sort_map: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
                his_labels_sort_map[..n].sort_by_key(|i| other.labels[*i as usize].key());
                for i in 0..n {
                    if self.labels[labels_sort_map[i] as usize]
                        != other.labels[his_labels_sort_map[i] as usize]
                    {
                        return false;
                    }
                }
                true
            }
            n => {
                let mut labels_sort_map: Vec<usize> = (0..n).collect();
                labels_sort_map.sort_by_key(|i| self.labels[*i].key());
                let mut his_labels_sort_map: Vec<usize> = (0..n).collect();
                his_labels_sort_map.sort_by_key(|i| other.labels[*i].key());
                for i in 0..n {
                    if self.labels[labels_sort_map[i]] != other.labels[his_labels_sort_map[i]] {
                        return false;
                    }
                }
                true
            }
        }
    }
}

impl Eq for Key {}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        match (&self.name, self.labels.len()).cmp(&(&other.name, other.labels.len())) {
            cmp::Ordering::Less => return cmp::Ordering::Less,
            cmp::Ordering::Equal => {}
            cmp::Ordering::Greater => return cmp::Ordering::Greater,
        }
        match self.labels.len() {
            0 => cmp::Ordering::Equal,
            1 => self.labels[0].cmp(&other.labels[0]),
            n if n < 8 => {
                let mut labels_sort_map: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
                labels_sort_map[..n].sort_by_key(|i| self.labels[*i as usize].key());
                let mut his_labels_sort_map: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
                his_labels_sort_map[..n].sort_by_key(|i| other.labels[*i as usize].key());
                for i in 0..n {
                    match self.labels[labels_sort_map[i] as usize]
                        .cmp(&other.labels[his_labels_sort_map[i] as usize])
                    {
                        cmp::Ordering::Less => return cmp::Ordering::Less,
                        cmp::Ordering::Equal => {}
                        cmp::Ordering::Greater => return cmp::Ordering::Greater,
                    }
                }
                cmp::Ordering::Equal
            }
            n => {
                let mut labels_sort_map: Vec<usize> = (0..n).collect();
                labels_sort_map.sort_by_key(|i| self.labels[*i].key());
                let mut his_labels_sort_map: Vec<usize> = (0..n).collect();
                his_labels_sort_map.sort_by_key(|i| other.labels[*i].key());
                for i in 0..n {
                    match self.labels[labels_sort_map[i]].cmp(&other.labels[his_labels_sort_map[i]])
                    {
                        cmp::Ordering::Less => return cmp::Ordering::Less,
                        cmp::Ordering::Equal => {}
                        cmp::Ordering::Greater => return cmp::Ordering::Greater,
                    }
                }
                cmp::Ordering::Equal
            }
        }
    }
}

impl Hash for Key {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        // MUST only call `write_u64(self.hash)`. The companion no-op `metrics_util::common::KeyHasher`
        // (and the no-op fast path in the deprecated `metrics::KeyHasher`) depend on this being the
        // sole write call — any byte write would route through a byte hasher and produce a value
        // that disagrees with `Key::get_hash()` / `Hashable::hashable(&key)`, causing duplicate
        // entries in `metrics_util::registry::Registry`'s internal HashMap after resize (#694).
        state.write_u64(self.hash)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.labels.is_empty() {
            write!(f, "Key({})", self.name.as_str())
        } else {
            write!(f, "Key({}, [", self.name.as_str())?;
            let mut first = true;
            for label in self.labels.as_ref() {
                if first {
                    write!(f, "{} = {}", label.0, label.1)?;
                    first = false;
                } else {
                    write!(f, ", {} = {}", label.0, label.1)?;
                }
            }
            write!(f, "])")
        }
    }
}

impl<T> From<T> for Key
where
    T: Into<KeyName>,
{
    fn from(name: T) -> Self {
        Self::from_name(name)
    }
}

impl<N, L> From<(N, L)> for Key
where
    N: Into<KeyName>,
    L: IntoLabels,
{
    fn from(parts: (N, L)) -> Self {
        Self::from_parts(parts.0, parts.1)
    }
}

#[cfg(test)]
mod tests {
    use super::Key;
    use crate::{KeyName, Label};
    use std::{cmp, collections::HashMap, ops::Deref, sync::Arc};

    static BORROWED_NAME: &str = "name";
    static FOOBAR_NAME: &str = "foobar";
    static BORROWED_BASIC: Key = Key::from_static_name(BORROWED_NAME);
    static LABELS: [Label; 1] = [Label::from_static_parts("key", "value")];
    static BORROWED_LABELS: Key = Key::from_static_parts(BORROWED_NAME, &LABELS);

    #[test]
    fn test_key_ord_and_partialord() {
        let keys_expected: Vec<Key> =
            vec![Key::from_name("aaaa"), Key::from_name("bbbb"), Key::from_name("cccc")];

        let keys_unsorted: Vec<Key> =
            vec![Key::from_name("bbbb"), Key::from_name("cccc"), Key::from_name("aaaa")];

        let keys = {
            let mut keys = keys_unsorted.clone();
            keys.sort();
            keys
        };
        assert_eq!(keys, keys_expected);

        let keys = {
            let mut keys = keys_unsorted.clone();
            keys.sort_by(|a, b| a.partial_cmp(b).unwrap());
            keys
        };
        assert_eq!(keys, keys_expected);
    }

    #[test]
    fn test_key_ord_total() {
        let mut keys: Vec<_> = (1..16)
            .flat_map(|i| {
                let names: Vec<Label> =
                    (0..i).map(|s| Label::new(format!("{}", s), format!("{}", s))).collect();
                let key1 = Key::from_parts("key", names.clone());

                // test that changing the order doesn't affect the value
                let mut names_alt = names.clone();
                names_alt.rotate_left(1);
                let key2 = Key::from_parts("key", names_alt);
                assert_eq!(key1, key2);

                // check that the first element affects the order
                names_alt = names.clone();
                names_alt[0] = Label::new("x".to_string(), "0".to_string());
                let key3 = Key::from_parts("key", names_alt);
                assert_ne!(key1, key3);

                // check that the last element affects the order
                names_alt = names.clone();
                names_alt[i - 1] = Label::new("x".to_string(), "0".to_string());
                let key4 = Key::from_parts("key", names_alt);
                assert_ne!(key1, key4);

                [key1, key2, key3, key4]
            })
            .collect();
        keys.push(Key::from_parts("key", vec![]));
        keys.sort();
        for i in 0..keys.len() {
            for j in 0..keys.len() {
                // check that the order is a total order
                match (keys[i] == keys[j], keys[i].cmp(&keys[j])) {
                    (true, cmp::Ordering::Equal) => {}
                    (true, cmp::Ordering::Less) => panic!("at {} {} equal keys compared lt", i, j),
                    (true, cmp::Ordering::Greater) => {
                        panic!("at {} {} equal keys compared gt", i, j)
                    }
                    (false, cmp::Ordering::Equal) => {
                        panic!("at {} {} unequal keys compared equal", i, j)
                    }
                    (false, cmp::Ordering::Less) => assert!(i < j),
                    (false, cmp::Ordering::Greater) => assert!(i > j),
                }
            }
        }
    }

    #[test]
    fn test_key_eq_and_hash() {
        let mut keys = HashMap::new();

        let owned_basic: Key = Key::from_name("name");
        assert_eq!(&owned_basic, &BORROWED_BASIC);

        let previous = keys.insert(owned_basic, 42);
        assert!(previous.is_none());

        let previous = keys.get(&BORROWED_BASIC);
        assert_eq!(previous, Some(&42));

        let labels = LABELS.to_vec();
        let owned_labels = Key::from_parts(BORROWED_NAME, labels);
        assert_eq!(&owned_labels, &BORROWED_LABELS);

        let previous = keys.insert(owned_labels, 43);
        assert!(previous.is_none());

        let previous = keys.get(&BORROWED_LABELS);
        assert_eq!(previous, Some(&43));

        let basic: Key = "constant_key".into();
        let cloned_basic = basic.clone();
        assert_eq!(basic, cloned_basic);

        for i in 1..16 {
            let names: Vec<Label> =
                (0..i).map(|s| Label::new(format!("{}", s), format!("{}", s))).collect();
            let key1 = Key::from_parts("key", names.clone());
            let mut names_alt = names.clone();

            // test that changing the order doesn't affect the hash
            names_alt.rotate_left(1);
            let key2 = Key::from_parts("key", names_alt);
            assert_eq!(key1, key2);
            assert_eq!(key1.get_hash(), key2.get_hash());

            // check that the first element affects the hash
            names_alt = names.clone();
            names_alt[0] = Label::new("x".to_string(), "0".to_string());
            let key2 = Key::from_parts("key", names_alt);
            assert_ne!(key1, key2);
            assert_ne!(key1.get_hash(), key2.get_hash());

            // check that the last element affects the hash
            names_alt = names.clone();
            names_alt[i - 1] = Label::new("x".to_string(), "0".to_string());
            let key2 = Key::from_parts("key", names_alt);
            assert_ne!(key1, key2);
            assert_ne!(key1.get_hash(), key2.get_hash());

            // check that it differs from a key with no labels
            let key2 = Key::from_parts("key", vec![]);
            assert_ne!(key1, key2);
            assert_ne!(key1.get_hash(), key2.get_hash());
        }
    }

    #[test]
    fn test_key_data_proper_display() {
        let key1 = Key::from_name("foobar");
        let result1 = key1.to_string();
        assert_eq!(result1, "Key(foobar)");

        let key2 = Key::from_parts(FOOBAR_NAME, vec![Label::new("system", "http")]);
        let result2 = key2.to_string();
        assert_eq!(result2, "Key(foobar, [system = http])");

        let key3 = Key::from_parts(
            FOOBAR_NAME,
            vec![Label::new("system", "http"), Label::new("user", "joe")],
        );
        let result3 = key3.to_string();
        assert_eq!(result3, "Key(foobar, [system = http, user = joe])");

        let key4 = Key::from_parts(
            FOOBAR_NAME,
            vec![
                Label::new("black", "black"),
                Label::new("lives", "lives"),
                Label::new("matter", "matter"),
            ],
        );
        let result4 = key4.to_string();
        assert_eq!(result4, "Key(foobar, [black = black, lives = lives, matter = matter])");
    }

    #[test]
    fn test_key_name_equality() {
        static KEY_NAME: &str = "key_name";

        let borrowed_const = KeyName::from_const_str(KEY_NAME);
        let borrowed_nonconst = KeyName::from(KEY_NAME);
        let owned = KeyName::from(KEY_NAME.to_owned());

        let shared_arc = Arc::from(KEY_NAME);
        let shared = KeyName::from(Arc::clone(&shared_arc));

        assert_eq!(borrowed_const, borrowed_nonconst);
        assert_eq!(borrowed_const.as_str(), borrowed_nonconst.as_str());
        assert_eq!(borrowed_const, owned);
        assert_eq!(borrowed_const.as_str(), owned.as_str());
        assert_eq!(borrowed_const, shared);
        assert_eq!(borrowed_const.as_str(), shared.as_str());
    }

    #[test]
    fn test_shared_key_name_drop_logic() {
        let shared_arc = Arc::from("foo");
        let shared = KeyName::from(Arc::clone(&shared_arc));

        assert_eq!(shared_arc.deref(), shared.as_str());

        assert_eq!(Arc::strong_count(&shared_arc), 2);
        drop(shared);
        assert_eq!(Arc::strong_count(&shared_arc), 1);

        let shared_weak = Arc::downgrade(&shared_arc);
        assert_eq!(Arc::strong_count(&shared_arc), 1);

        let shared = KeyName::from(Arc::clone(&shared_arc));
        assert_eq!(shared_arc.deref(), shared.as_str());
        assert_eq!(Arc::strong_count(&shared_arc), 2);

        drop(shared_arc);
        assert_eq!(shared_weak.strong_count(), 1);

        drop(shared);
        assert_eq!(shared_weak.strong_count(), 0);
    }

    #[test]
    fn test_key_to_retained_preserves_equality_and_hash() {
        let key = Key::from_parts(
            String::from("retained"),
            vec![Label::new(String::from("service"), String::from("api"))],
        );

        let retained = key.to_retained();

        assert_eq!(key, retained);
        assert_eq!(key.get_hash(), retained.get_hash());
        assert_eq!(retained, retained.clone());
    }

    #[test]
    fn test_buildhasherdefault_keyhasher_agrees_with_get_hash() {
        use std::hash::{BuildHasher, BuildHasherDefault};
        #[allow(deprecated)]
        use crate::KeyHasher;

        // The Registry in `metrics-util` versions 0.19.0..=0.20.1 uses
        // `BuildHasherDefault<metrics::KeyHasher>` as the HashMap's BuildHasher, while looking
        // up entries via `Hashable::hashable(&key)` (which returns `Key::get_hash()` directly).
        // If those two paths produce different u64s, hashbrown's resize-time rebucketing places
        // entries at a different bucket than the lookup queries, and the Registry inserts a
        // fresh entry on every call. See https://github.com/metrics-rs/metrics/issues/694.
        let key = Key::from_parts("regression", vec![Label::new("a", "1")]);
        #[allow(deprecated)]
        let via_buildhasher = BuildHasherDefault::<KeyHasher>::default().hash_one(&key);
        assert_eq!(via_buildhasher, key.get_hash());
    }

    #[test]
    fn test_keyhasher_byte_mode_distinguishes_inputs() {
        use std::hash::{BuildHasher, BuildHasherDefault};
        #[allow(deprecated)]
        use crate::KeyHasher;

        // Validates the byte-mode fallback path used by `metrics_util::DefaultHashable<H>` in
        // `metrics-util 0.19.x`: when `Hash::hash` writes bytes (not just `write_u64`), the
        // hasher must still produce deterministic, input-distinguishing output.
        #[allow(deprecated)]
        let bh = BuildHasherDefault::<KeyHasher>::default();
        let h_foo_1 = bh.hash_one("foo");
        let h_foo_2 = bh.hash_one("foo");
        let h_bar = bh.hash_one("bar");
        assert_eq!(h_foo_1, h_foo_2);
        assert_ne!(h_foo_1, h_bar);
    }
}
