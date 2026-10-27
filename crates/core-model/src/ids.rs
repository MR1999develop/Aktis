//! Identifiers. Each is a domain primitive: it exists only if valid.

use std::fmt;

/// Why a string was refused as an identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    /// Empty string.
    Empty,
    /// Longer than [`MAX_ID_LEN`] bytes.
    TooLong,
    /// Contains a character outside `a-z`, `0-9`, `_`, `.`, or does not start with a letter.
    InvalidCharacter,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "identifier is empty"),
            Self::TooLong => write!(f, "identifier is longer than {MAX_ID_LEN} characters"),
            Self::InvalidCharacter => {
                write!(f, "identifier must start with a-z and contain only a-z, 0-9, '_' and '.'")
            }
        }
    }
}

impl std::error::Error for IdError {}

/// Maximum length of a [`ToolId`] or [`ParamId`], in bytes.
const MAX_ID_LEN: usize = 64;

/// Length is checked before characters, so an enormous input is refused at once.
fn validate(s: &str) -> Result<(), IdError> {
    if s.is_empty() {
        return Err(IdError::Empty);
    }
    if s.len() > MAX_ID_LEN {
        return Err(IdError::TooLong);
    }
    let mut chars = s.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
    if first_ok && rest_ok { Ok(()) } else { Err(IdError::InvalidCharacter) }
}

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Validates `s` and wraps it.
            ///
            /// # Errors
            /// [`IdError`] when `s` is empty, too long or has characters outside
            /// `a-z 0-9 _ .` (first character a letter).
            pub fn new(s: &str) -> Result<Self, IdError> {
                validate(s)?;
                Ok(Self(s.to_owned()))
            }

            /// The identifier as text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = IdError;
            fn try_from(s: &str) -> Result<Self, IdError> {
                Self::new(s)
            }
        }
    };
}

string_id!(
    /// Stable identifier of a tool (`"exposure"`). It is the key in saved
    /// recipes, so it never changes once a tool ships.
    ToolId
);

string_id!(
    /// Identifier of one parameter inside a tool (`"exposure"`, `"shadows"`).
    ParamId
);

string_id!(
    /// Stable identifier of an AI model and its weights (`"sam2.1_small"`).
    /// A recipe records which model produced each AI mask, so a mask can be
    /// recomputed with the same model, or flagged when it is not installed.
    ModelId
);

/// Identifier of a node inside one graph. Never reused within that graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub(crate) u32);

impl NodeId {
    /// The raw number, for display and serialisation.
    #[must_use]
    pub fn get(self) -> u32 {
        self.0
    }

    /// Rebuilds an id read back from storage. Only the graph decides whether
    /// the node exists.
    #[must_use]
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_lowercase_names_are_valid_ids() {
        assert_eq!(ToolId::new("exposure").map(|t| t.to_string()), Ok("exposure".to_owned()));
        assert!(ParamId::new("color_balance.shadow_hue").is_ok());
    }

    #[test]
    fn ids_refuse_empty_uppercase_leading_digit_and_spaces() {
        assert_eq!(ToolId::new(""), Err(IdError::Empty));
        assert_eq!(ToolId::new("Exposure"), Err(IdError::InvalidCharacter));
        assert_eq!(ToolId::new("1exposure"), Err(IdError::InvalidCharacter));
        assert_eq!(ToolId::new("exp osure"), Err(IdError::InvalidCharacter));
        assert_eq!(ToolId::new("espos\u{e9}"), Err(IdError::InvalidCharacter));
    }

    #[test]
    fn an_enormous_id_is_refused_for_length_before_anything_else() {
        let huge = "A".repeat(100_000);
        assert_eq!(ParamId::new(&huge), Err(IdError::TooLong));
        assert!(ParamId::new(&"a".repeat(64)).is_ok());
        assert_eq!(ParamId::new(&"a".repeat(65)), Err(IdError::TooLong));
    }
}
