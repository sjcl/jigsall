//! Display metadata, independent of session ownership and platform identities.
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

pub const MAX_PLAYER_DISPLAY_NAME_CHARS: usize = 32;
pub const MAX_PLAYER_DISPLAY_NAME_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerDisplayName(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayNameError {
    Empty,
    TooManyChars,
    TooManyBytes,
    ForbiddenCharacter,
}
impl fmt::Display for DisplayNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "display name is empty",
            Self::TooManyChars => "display name exceeds 32 characters",
            Self::TooManyBytes => "display name exceeds 128 UTF-8 bytes",
            Self::ForbiddenCharacter => {
                "display name contains a control or bidirectional formatting character"
            }
        })
    }
}
impl std::error::Error for DisplayNameError {}

impl PlayerDisplayName {
    pub fn from_user_input(input: &str) -> Result<Self, DisplayNameError> {
        Self::optional_from_user_input(input)?.ok_or(DisplayNameError::Empty)
    }

    pub fn optional_from_user_input(input: &str) -> Result<Option<Self>, DisplayNameError> {
        // Check before trimming: newlines and forbidden separators are never names.
        if input.chars().any(|c| {
            c.is_control()
                || matches!(c,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}'
            | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        }) {
            return Err(DisplayNameError::ForbiddenCharacter);
        }
        let input = input.trim();
        if input.is_empty() {
            return Ok(None);
        }
        if input.len() > MAX_PLAYER_DISPLAY_NAME_BYTES {
            return Err(DisplayNameError::TooManyBytes);
        }
        if input.chars().count() > MAX_PLAYER_DISPLAY_NAME_CHARS {
            return Err(DisplayNameError::TooManyChars);
        }
        Ok(Some(Self(input.to_owned())))
    }
}
impl AsRef<str> for PlayerDisplayName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for PlayerDisplayName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Serialize for PlayerDisplayName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for PlayerDisplayName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NameVisitor;
        impl de::Visitor<'_> for NameVisitor {
            type Value = PlayerDisplayName;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a bounded, nonempty player display name")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                // Postcard supplies a borrowed slice: reject before any owned allocation.
                if value.len() > MAX_PLAYER_DISPLAY_NAME_BYTES {
                    return Err(E::custom(DisplayNameError::TooManyBytes));
                }
                PlayerDisplayName::from_user_input(value).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(NameVisitor)
    }
}
