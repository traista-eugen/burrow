//! [`Timestamp`]: an instant with millisecond precision that always serializes as
//! `YYYY-MM-DDTHH:MM:SS.mmmZ`.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An instant in UTC, with millisecond precision (D-011).
///
/// On the wire it is always an RFC 3339 string with exactly three fractional digits and a `Z`,
/// for example `2026-11-06T14:03:22.418Z`. Sub-millisecond precision is dropped when a value is
/// made, so a timestamp survives any number of round trips unchanged.
///
/// Parsing is lenient: any RFC 3339 instant is accepted, including other offsets and more or
/// fewer fractional digits. It is converted to UTC and truncated to the millisecond.
///
/// The supported range is `0000-01-01T00:00:00.000Z` to `9999-12-30T22:00:00.999Z` (the end of
/// `jiff`'s range), so the year always has four digits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(jiff::Timestamp);

/// The earliest supported instant, `0000-01-01T00:00:00.000Z`, in Unix milliseconds.
const MIN_UNIX_MILLIS: i64 = -62_167_219_200_000;

/// A value outside [`Timestamp`]'s range, or a string that isn't an RFC 3339 instant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimestampError(String);

impl fmt::Display for TimestampError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TimestampError {}

impl Timestamp {
    /// The current time, truncated to the millisecond.
    pub fn now() -> Self {
        Self::from_jiff(jiff::Timestamp::now()).expect("the system clock is within 0000..=9999")
    }

    /// Makes a timestamp from milliseconds since the Unix epoch.
    pub fn from_unix_millis(millis: i64) -> Result<Self, TimestampError> {
        if millis < MIN_UNIX_MILLIS {
            return Err(TimestampError(format!(
                "{millis} ms is before 0000-01-01T00:00:00.000Z"
            )));
        }
        jiff::Timestamp::from_millisecond(millis)
            .map(Self)
            .map_err(|error| TimestampError(error.to_string()))
    }

    /// Milliseconds since the Unix epoch. Lossless: `from_unix_millis(t.as_unix_millis()) == t`.
    pub fn as_unix_millis(self) -> i64 {
        self.0.as_millisecond()
    }

    /// Converts from `jiff`, rounding down to the millisecond.
    pub fn from_jiff(timestamp: jiff::Timestamp) -> Result<Self, TimestampError> {
        // Floor, not `as_millisecond`'s truncation toward zero, so instants before 1970 round
        // down too.
        let millis = timestamp
            .as_second()
            .checked_mul(1000)
            .and_then(|ms| {
                ms.checked_add(i64::from(
                    timestamp.subsec_nanosecond().div_euclid(1_000_000),
                ))
            })
            .ok_or_else(|| TimestampError(format!("{timestamp} is out of range")))?;
        Self::from_unix_millis(millis)
    }

    /// The same instant as a `jiff::Timestamp`, for arithmetic and local-time display.
    pub fn as_jiff(self) -> jiff::Timestamp {
        self.0
    }
}

impl fmt::Display for Timestamp {
    /// Writes the wire format, `YYYY-MM-DDTHH:MM:SS.mmmZ`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3}", self.0)
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timestamp({self})")
    }
}

impl FromStr for Timestamp {
    type Err = TimestampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parsed: jiff::Timestamp = s
            .parse()
            .map_err(|error| TimestampError(format!("invalid timestamp {s:?}: {error}")))?;
        Self::from_jiff(parsed)
    }
}

impl TryFrom<jiff::Timestamp> for Timestamp {
    type Error = TimestampError;

    fn try_from(timestamp: jiff::Timestamp) -> Result<Self, Self::Error> {
        Self::from_jiff(timestamp)
    }
}

impl From<Timestamp> for jiff::Timestamp {
    fn from(timestamp: Timestamp) -> Self {
        timestamp.0
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Timestamp {
    fn schema_name() -> Cow<'static, str> {
        "Timestamp".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Timestamp").into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "An instant in UTC: RFC 3339 with exactly three fractional digits and `Z`, for example `2026-11-06T14:03:22.418Z`.",
            "type": "string",
            "format": "date-time",
            "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\\.[0-9]{3}Z$",
        })
    }
}
