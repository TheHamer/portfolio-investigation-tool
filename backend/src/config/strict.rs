//! Strict scalar deserialisers shared by the configuration sections.
//!
//! The `config` crate's typed conversions are lenient: asked for an integer it
//! rounds `5.5` to 6 and turns `true` into 1; asked for a boolean it accepts
//! 1 or "yes". These helpers use `deserialize_any`, which exposes the value as
//! written, so only whole numbers and real booleans are accepted. Environment
//! variables always arrive as strings, so the string forms "250" and "true" are
//! accepted too.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, Deserializer, Unexpected, Visitor};

/// A non-negative whole number that fits in `T`.
pub(crate) fn strict_uint<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<u64>,
{
    struct WholeNumber<T>(PhantomData<T>);

    impl<T: TryFrom<u64>> Visitor<'_> for WholeNumber<T> {
        type Value = T;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "a non-negative whole number that fits in {}",
                std::any::type_name::<T>()
            )
        }

        fn visit_u64<E: de::Error>(self, value: u64) -> Result<T, E> {
            T::try_from(value).map_err(|_| E::invalid_value(Unexpected::Unsigned(value), &self))
        }

        fn visit_i64<E: de::Error>(self, value: i64) -> Result<T, E> {
            let unsigned = u64::try_from(value)
                .map_err(|_| E::invalid_value(Unexpected::Signed(value), &self))?;
            self.visit_u64(unsigned)
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<T, E> {
            let parsed: u64 = value
                .parse()
                .map_err(|_| E::invalid_value(Unexpected::Str(value), &self))?;
            self.visit_u64(parsed)
        }
    }

    deserializer.deserialize_any(WholeNumber(PhantomData))
}

/// `true` or `false`: a TOML boolean, or the exact string from the environment.
pub(crate) fn strict_bool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    struct Boolean;

    impl Visitor<'_> for Boolean {
        type Value = bool;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("true or false")
        }

        fn visit_bool<E: de::Error>(self, value: bool) -> Result<bool, E> {
            Ok(value)
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<bool, E> {
            match value {
                "true" => Ok(true),
                "false" => Ok(false),
                other => Err(E::invalid_value(Unexpected::Str(other), &self)),
            }
        }
    }

    deserializer.deserialize_any(Boolean)
}
