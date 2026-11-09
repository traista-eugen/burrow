//! `open_enum!`: string enums that a newer peer may extend within a protocol version.
//!
//! A value this version doesn't know decodes into `Other(String)` and is sent back unchanged, so
//! an older client never fails on a newer server's value (protocol.md, "Forward compatibility").
//! Closed enums, whose values only change with the protocol version, use plain serde derives.

/// Joins `#[doc]` lines collected by a macro into one description.
pub(crate) fn doc_text(lines: &[&str]) -> String {
    lines
        .iter()
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// Defines an open string enum.
///
/// ```text
/// open_enum! {
///     /// What the enum is.
///     pub enum EntityKind {
///         /// A project.
///         Project = "project",
///     }
/// }
/// ```
///
/// Generates the enum plus an `Other(String)` variant, `as_str`, `is_known`, `From<&str>`,
/// `From<String>`, `Display`, serde impls (a plain JSON string) and a `JsonSchema` impl (a
/// `oneOf` of the known values, each with its description, and any other string).
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
        /// This is an open enum: a value this version doesn't know is kept in `Other` and sent
        /// back unchanged. Build values with `From<&str>` so known strings map to their variants.
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $(
                $(#[doc = $vdoc])*
                $variant,
            )+
            /// A value this version of the protocol doesn't know, kept as sent.
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

        impl ::schemars::JsonSchema for $name {
            fn schema_name() -> ::std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }

            fn schema_id() -> ::std::borrow::Cow<'static, str> {
                concat!(module_path!(), "::", stringify!($name)).into()
            }

            fn json_schema(_: &mut ::schemars::SchemaGenerator) -> ::schemars::Schema {
                let mut one_of = vec![
                    $(
                        ::serde_json::json!({
                            "const": $value,
                            "description": $crate::open_enum::doc_text(&[$($vdoc),*]),
                        }),
                    )+
                ];
                one_of.push(::serde_json::json!({
                    "type": "string",
                    "not": { "enum": Self::KNOWN },
                    "description": "A value added by a newer protocol revision. Keep it and pass it on.",
                }));
                ::schemars::json_schema!({
                    "description": $crate::open_enum::doc_text(&[$($doc),*]),
                    "type": "string",
                    "oneOf": one_of,
                })
            }
        }
    };
}
