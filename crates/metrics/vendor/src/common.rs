use std::hash::Hasher;

use rapidhash::fast::RapidHasher;

use crate::cow::Cow;

/// An allocation-optimized string.
///
/// `SharedString` uses a custom copy-on-write implementation that is optimized for metric keys,
/// providing ergonomic sharing of single instances, or slices, of strings and labels. This
/// copy-on-write implementation is optimized to allow for constant-time construction (using static
/// values), as well as accepting owned values and values shared through [`Arc<T>`](std::sync::Arc).
///
/// End users generally will not need to interact with this type directly, as the top-level macros
/// (`counter!`, etc), as well as the various conversion implementations
/// ([`From<T>`](std::convert::From)), generally allow users to pass whichever variant of a value
/// (static, owned, shared) is best for them.
pub type SharedString = Cow<'static, str>;

/// Key-specific hashing algorithm.
///
/// Deprecated in favor of a no-hash based implementation in `metrics-util::common::KeyHasher`.
///
/// This hasher operates in two modes. When only `write_u64` is called — the path taken by
/// [`Key`][crate::Key]'s [`Hash`][std::hash::Hash] impl, which writes the pre-computed key hash
/// via `write_u64(self.hash)` — `finish` returns that value verbatim, matching `Hashable::hashable(&key)`
/// and the no-op `metrics_util::common::KeyHasher`. When arbitrary bytes are written via `write`
/// (or any of the typed `write_*` methods that route through `write`), this hasher falls back to
/// hashing with rapidhash - <https://github.com/hoxxep/rapidhash> - preserving the previous
/// byte-hashing behavior for callers like `metrics_util::DefaultHashable<H>` in `metrics-util 0.19.x`.
#[deprecated(since = "0.24.4", note = "Use `metrics-util::common::KeyHasher` instead.")]
pub struct KeyHasher(KeyHasherState);

enum KeyHasherState {
    Empty,
    PreHashed(u64),
    Bytes(RapidHasher<'static>),
}

#[allow(deprecated)]
impl std::fmt::Debug for KeyHasher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyHasher").finish_non_exhaustive()
    }
}

#[allow(deprecated)]
impl Default for KeyHasher {
    fn default() -> Self {
        KeyHasher(KeyHasherState::Empty)
    }
}

#[allow(deprecated)]
impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        match &self.0 {
            KeyHasherState::Empty => 0,
            KeyHasherState::PreHashed(h) => *h,
            KeyHasherState::Bytes(h) => h.finish(),
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        // Any byte write transitions to byte mode. If a prior `write_u64` had stored a
        // pre-hashed value, fold it into the byte hasher first so multi-write hashing
        // remains well-defined and deterministic.
        let mut hasher = match std::mem::replace(&mut self.0, KeyHasherState::Empty) {
            KeyHasherState::Empty => RapidHasher::default_const(),
            KeyHasherState::PreHashed(prior) => {
                let mut h = RapidHasher::default_const();
                h.write_u64(prior);
                h
            }
            KeyHasherState::Bytes(h) => h,
        };
        hasher.write(bytes);
        self.0 = KeyHasherState::Bytes(hasher);
    }

    fn write_u64(&mut self, i: u64) {
        // The pre-hashed fast path: store the value directly so `finish` returns it verbatim.
        // If we've already transitioned to byte mode, stay there.
        self.0 = match std::mem::replace(&mut self.0, KeyHasherState::Empty) {
            KeyHasherState::Empty | KeyHasherState::PreHashed(_) => KeyHasherState::PreHashed(i),
            KeyHasherState::Bytes(mut h) => {
                h.write_u64(i);
                KeyHasherState::Bytes(h)
            }
        };
    }
}

/// Value of a gauge operation.
#[derive(Clone, Debug)]
pub enum GaugeValue {
    /// Sets the value of the gauge to this value.
    Absolute(f64),
    /// Increments the value of the gauge by this much.
    Increment(f64),
    /// Decrements the value of the gauge by this much.
    Decrement(f64),
}

impl GaugeValue {
    /// Updates an input value based on this gauge value.
    pub fn update_value(&self, input: f64) -> f64 {
        match self {
            GaugeValue::Absolute(val) => *val,
            GaugeValue::Increment(val) => input + val,
            GaugeValue::Decrement(val) => input - val,
        }
    }
}

/// Units for a given metric.
///
/// While metrics do not necessarily need to be tied to a particular unit to be recorded, some
/// downstream systems natively support defining units and so they can be specified during registration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Unit {
    /// Count.
    Count,
    /// Percentage.
    Percent,
    /// Seconds.
    ///
    /// One second is equal to 1000 milliseconds.
    Seconds,
    /// Milliseconds.
    ///
    /// One millisecond is equal to 1000 microseconds.
    Milliseconds,
    /// Microseconds.
    ///
    /// One microsecond is equal to 1000 nanoseconds.
    Microseconds,
    /// Nanoseconds.
    Nanoseconds,
    /// Tebibytes.
    ///
    /// One tebibyte is equal to 1024 gibibytes.
    Tebibytes,
    /// Gibibytes.
    ///
    /// One gibibyte is equal to 1024 mebibytes.
    Gibibytes,
    /// Mebibytes.
    ///
    /// One mebibyte is equal to 1024 kibibytes.
    Mebibytes,
    /// Kibibytes.
    ///
    /// One kibibyte is equal to 1024 bytes.
    Kibibytes,
    /// Bytes.
    Bytes,
    /// Terabits per second.
    ///
    /// One terabit is equal to 1000 gigabits.
    TerabitsPerSecond,
    /// Gigabits per second.
    ///
    /// One gigabit is equal to 1000 megabits.
    GigabitsPerSecond,
    /// Megabits per second.
    ///
    /// One megabit is equal to 1000 kilobits.
    MegabitsPerSecond,
    /// Kilobits per second.
    ///
    /// One kilobit is equal to 1000 bits.
    KilobitsPerSecond,
    /// Bits per second.
    BitsPerSecond,
    /// Count per second.
    CountPerSecond,
}

impl Unit {
    /// Gets the string form of this `Unit`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Unit::Count => "count",
            Unit::Percent => "percent",
            Unit::Seconds => "seconds",
            Unit::Milliseconds => "milliseconds",
            Unit::Microseconds => "microseconds",
            Unit::Nanoseconds => "nanoseconds",
            Unit::Tebibytes => "tebibytes",
            Unit::Gibibytes => "gibibytes",
            Unit::Mebibytes => "mebibytes",
            Unit::Kibibytes => "kibibytes",
            Unit::Bytes => "bytes",
            Unit::TerabitsPerSecond => "terabits_per_second",
            Unit::GigabitsPerSecond => "gigabits_per_second",
            Unit::MegabitsPerSecond => "megabits_per_second",
            Unit::KilobitsPerSecond => "kilobits_per_second",
            Unit::BitsPerSecond => "bits_per_second",
            Unit::CountPerSecond => "count_per_second",
        }
    }

    /// Gets the canonical string label for the given unit.
    ///
    /// For example, the canonical label for `Seconds` would be `s`, while for `Nanoseconds`,
    /// it would be `ns`.
    ///
    /// Not all units have a meaningful display label and so some may be empty.
    pub fn as_canonical_label(&self) -> &'static str {
        match self {
            Unit::Count => "",
            Unit::Percent => "%",
            Unit::Seconds => "s",
            Unit::Milliseconds => "ms",
            Unit::Microseconds => "μs",
            Unit::Nanoseconds => "ns",
            Unit::Tebibytes => "TiB",
            Unit::Gibibytes => "GiB",
            Unit::Mebibytes => "MiB",
            Unit::Kibibytes => "KiB",
            Unit::Bytes => "B",
            Unit::TerabitsPerSecond => "Tbps",
            Unit::GigabitsPerSecond => "Gbps",
            Unit::MegabitsPerSecond => "Mbps",
            Unit::KilobitsPerSecond => "kbps",
            Unit::BitsPerSecond => "bps",
            Unit::CountPerSecond => "/s",
        }
    }

    /// Converts the string representation of a unit back into `Unit` if possible.
    ///
    /// The value passed here should match the output of [`Unit::as_str`].
    pub fn from_string(s: &str) -> Option<Unit> {
        match s {
            "count" => Some(Unit::Count),
            "percent" => Some(Unit::Percent),
            "seconds" => Some(Unit::Seconds),
            "milliseconds" => Some(Unit::Milliseconds),
            "microseconds" => Some(Unit::Microseconds),
            "nanoseconds" => Some(Unit::Nanoseconds),
            "tebibytes" => Some(Unit::Tebibytes),
            "gibibytes" => Some(Unit::Gibibytes),
            "mebibytes" => Some(Unit::Mebibytes),
            "kibibytes" => Some(Unit::Kibibytes),
            "bytes" => Some(Unit::Bytes),
            "terabits_per_second" => Some(Unit::TerabitsPerSecond),
            "gigabits_per_second" => Some(Unit::GigabitsPerSecond),
            "megabits_per_second" => Some(Unit::MegabitsPerSecond),
            "kilobits_per_second" => Some(Unit::KilobitsPerSecond),
            "bits_per_second" => Some(Unit::BitsPerSecond),
            "count_per_second" => Some(Unit::CountPerSecond),
            _ => None,
        }
    }

    /// Whether or not this unit relates to the measurement of time.
    pub fn is_time_based(&self) -> bool {
        matches!(self, Unit::Seconds | Unit::Milliseconds | Unit::Microseconds | Unit::Nanoseconds)
    }

    /// Whether or not this unit relates to the measurement of data.
    pub fn is_data_based(&self) -> bool {
        matches!(
            self,
            Unit::Tebibytes
                | Unit::Gibibytes
                | Unit::Mebibytes
                | Unit::Kibibytes
                | Unit::Bytes
                | Unit::TerabitsPerSecond
                | Unit::GigabitsPerSecond
                | Unit::MegabitsPerSecond
                | Unit::KilobitsPerSecond
                | Unit::BitsPerSecond
        )
    }

    /// Whether or not this unit relates to the measurement of data rates.
    pub fn is_data_rate_based(&self) -> bool {
        matches!(
            self,
            Unit::TerabitsPerSecond
                | Unit::GigabitsPerSecond
                | Unit::MegabitsPerSecond
                | Unit::KilobitsPerSecond
                | Unit::BitsPerSecond
        )
    }
}

/// An object which can be converted into a `f64` representation.
///
/// This trait provides a mechanism for existing types, which have a natural representation
/// as a 64-bit floating-point number, to be transparently passed in when recording a histogram.
pub trait IntoF64 {
    /// Converts this object to its `f64` representation.
    fn into_f64(self) -> f64;
}

impl IntoF64 for f64 {
    fn into_f64(self) -> f64 {
        self
    }
}

impl IntoF64 for core::time::Duration {
    fn into_f64(self) -> f64 {
        self.as_secs_f64()
    }
}

into_f64!(i8, u8, i16, u16, i32, u32, f32);

/// Helper method to allow monomorphization of values passed to the `histogram!` macro.
#[doc(hidden)]
pub fn __into_f64<V: IntoF64>(value: V) -> f64 {
    value.into_f64()
}

macro_rules! into_f64 {
    ($($ty:ty),*) => {
        $(
            impl IntoF64 for $ty {
                fn into_f64(self) -> f64 {
                    f64::from(self)
                }
            }
        )*
    };
}

use into_f64;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{IntoF64, Unit};

    #[test]
    fn test_unit_conversions() {
        let all_variants = vec![
            Unit::Count,
            Unit::Percent,
            Unit::Seconds,
            Unit::Milliseconds,
            Unit::Microseconds,
            Unit::Nanoseconds,
            Unit::Tebibytes,
            Unit::Gibibytes,
            Unit::Mebibytes,
            Unit::Kibibytes,
            Unit::Bytes,
            Unit::TerabitsPerSecond,
            Unit::GigabitsPerSecond,
            Unit::MegabitsPerSecond,
            Unit::KilobitsPerSecond,
            Unit::BitsPerSecond,
            Unit::CountPerSecond,
        ];

        for variant in all_variants {
            let s = variant.as_str();
            let parsed = Unit::from_string(s);
            assert_eq!(Some(variant), parsed);
        }
    }

    #[test]
    fn into_f64() {
        fn test<T: IntoF64>(val: T) {
            assert!(!val.into_f64().is_nan());
        }

        test::<i8>(1);
        test::<u8>(1);
        test::<i16>(1);
        test::<u16>(1);
        test::<i32>(1);
        test::<u32>(1);
        test::<f32>(1.0);
        test::<f64>(1.0);
        test::<Duration>(Duration::from_secs(1));
    }
}
