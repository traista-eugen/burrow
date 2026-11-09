//! `open_enum!`: string enums that a newer Muse may extend.
//!
//! The MSP schema marks most of its enums `"x-msp-openness": "open"`: a client must accept values
//! it doesn't know (an unknown item kind is rendered generically, an unknown status is "terminal,
//! unknown"). A value this crate doesn't know decodes into `Other(String)` and encodes back
//! unchanged. Enums the schema leaves closed use plain serde derives.

/// Defines an open string enum.
///
/// ```text
/// open_enum! {
///     /// What the enum is.
///     pub enum ItemStatus {
///         /// Still running.
///         InProgress = "inProgress",
///     }
/// }
/// ```
///
/// Generates the enum plus an `Other(String)` variant, `as_str`, `is_known`, `From<&str>`,
/// `From<String>`, `Display` and serde impls (a plain JSON string). In unit tests it also
/// implements `schemars::JsonSchema` as a plain string, for the schema checks.
macro_rules! open_enum {
    (
        $(#[doc = $doc:literal])*
        pub enum $name:ident {
            $(
                $(#[doc = $vdoc:literal])*
                $variant:ident = $value:literal,
            )+
        }
    ) => {
        $(#[doc = $doc])*
        ///
        /// Open: a value this version doesn't know is kept in `Other` and sent back unchanged.
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $(
                $(#[doc = $vdoc])*
                $variant,
            )+
            /// A value this version doesn't know, kept as sent.
            Other(String),
        }

        impl $name {
            /// Every value this version knows, in declaration order.
            pub const KNOWN: &'static [&'static str] = &[$($value),+];

            /// The wire value.
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $value,)+
                    Self::Other(value) => value,
                }
            }

            /// Whether this is one of the values this version knows.
            pub fn is_known(&self) -> bool {
                !matches!(self, Self::Other(_))
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                match value {
                    $($value => Self::$variant,)+
                    other => Self::Other(other.to_owned()),
                }
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                match value.as_str() {
                    $($value => Self::$variant,)+
                    _ => Self::Other(value),
                }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                <String as ::serde::Deserialize>::deserialize(deserializer).map(Self::from)
            }
        }

        #[cfg(test)]
        impl ::schemars::JsonSchema for $name {
            fn schema_name() -> ::std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut ::schemars::SchemaGenerator) -> ::schemars::Schema {
                ::schemars::json_schema!({ "type": "string" })
            }
        }
    };
}

#[cfg(test)]
mod tests {
    open_enum! {
        /// A test enum.
        pub enum Colour {
            /// Red.
            Red = "red",
            /// Dark blue.
            DarkBlue = "darkBlue",
        }
    }

    #[test]
    fn known_values_map_to_variants_and_back() {
        assert_eq!(Colour::from("darkBlue"), Colour::DarkBlue);
        assert_eq!(Colour::DarkBlue.as_str(), "darkBlue");
        assert_eq!(serde_json::to_value(Colour::Red).unwrap(), "red");
        assert!(Colour::Red.is_known());
        assert_eq!(Colour::KNOWN, ["red", "darkBlue"]);
    }

    #[test]
    fn unknown_values_survive_a_round_trip() {
        let colour: Colour = serde_json::from_str("\"ultraviolet\"").unwrap();
        assert_eq!(colour, Colour::Other("ultraviolet".to_owned()));
        assert!(!colour.is_known());
        assert_eq!(serde_json::to_string(&colour).unwrap(), "\"ultraviolet\"");
    }
}
