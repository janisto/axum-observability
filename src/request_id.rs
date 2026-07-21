use std::{error::Error, fmt, str::FromStr};

/// A validated request identifier.
///
/// Values contain between 1 and [`MAX_LEN`](Self::MAX_LEN) ASCII
/// URI-unreserved bytes.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RequestId(Box<str>);

impl RequestId {
    /// Maximum accepted request-ID length in bytes.
    pub const MAX_LEN: usize = 128;

    /// Parses and validates a request identifier.
    ///
    /// # Errors
    ///
    /// Returns the first deterministic baseline validation failure without
    /// retaining or echoing the rejected value.
    ///
    /// # Examples
    ///
    /// ```
    /// use axum_observability::RequestId;
    ///
    /// let request_id = RequestId::parse("request-42")?;
    /// assert_eq!(request_id.as_str(), "request-42");
    /// # Ok::<(), axum_observability::InvalidRequestId>(())
    /// ```
    pub fn parse(value: &str) -> Result<Self, InvalidRequestId> {
        validate(value)?;
        Ok(Self(value.into()))
    }

    pub(crate) fn from_native_header(value: &str) -> Option<Self> {
        native_field_content(value).then(|| Self(value.into()))
    }

    /// Returns the validated identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub(crate) fn native_field_content(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || matches!(bytes.first(), Some(b' ' | b'\t'))
        || matches!(bytes.last(), Some(b' ' | b'\t'))
    {
        return false;
    }
    bytes
        .iter()
        .all(|byte| *byte == b'\t' || *byte >= 0x20 && *byte != 0x7f)
}

impl AsRef<str> for RequestId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for RequestId {
    type Err = InvalidRequestId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for RequestId {
    type Error = InvalidRequestId;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for RequestId {
    type Error = InvalidRequestId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate(&value)?;
        Ok(Self(value.into_boxed_str()))
    }
}

/// Reason a request identifier failed baseline validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InvalidRequestId {
    /// The identifier was empty.
    Empty,
    /// The identifier exceeded [`RequestId::MAX_LEN`] bytes.
    TooLong {
        /// Rejected byte length.
        length: usize,
    },
    /// The identifier contained a byte outside the URI-unreserved set.
    InvalidCharacter {
        /// Byte index of the first invalid character.
        index: usize,
    },
}

impl fmt::Display for InvalidRequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("request ID must not be empty"),
            Self::TooLong { length } => {
                write!(formatter, "request ID length {length} exceeds 128 bytes")
            }
            Self::InvalidCharacter { index } => write!(
                formatter,
                "request ID contains an invalid character at byte index {index}"
            ),
        }
    }
}

impl Error for InvalidRequestId {}

fn validate(value: &str) -> Result<(), InvalidRequestId> {
    if value.is_empty() {
        return Err(InvalidRequestId::Empty);
    }
    if value.len() > RequestId::MAX_LEN {
        return Err(InvalidRequestId::TooLong {
            length: value.len(),
        });
    }
    if let Some(index) = value.bytes().position(|byte| {
        !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
    }) {
        return Err(InvalidRequestId::InvalidCharacter { index });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use super::{InvalidRequestId, RequestId, native_field_content};

    #[test]
    fn native_field_content_admits_internal_htab_but_rejects_controls() {
        assert!(native_field_content("tenant\trequest"));
        for value in ["tenant\0request", "tenant\x1frequest", "tenant\x7frequest"] {
            assert!(!native_field_content(value), "admitted {value:?}");
        }
    }

    #[test]
    fn accepts_exact_length_boundaries_and_conversion_forms() {
        let one = RequestId::parse("a").expect("one byte");
        assert_eq!(one.as_str(), "a");
        assert_eq!(one.as_ref(), "a");

        let maximum = "aZ09-._~".repeat(16);
        assert_eq!(maximum.len(), RequestId::MAX_LEN);
        assert_eq!(
            RequestId::from_str(&maximum).expect("from str").as_str(),
            maximum
        );
        assert_eq!(
            RequestId::try_from(maximum.as_str())
                .expect("borrowed")
                .as_str(),
            maximum
        );
        assert_eq!(
            RequestId::try_from(maximum.clone())
                .expect("owned")
                .as_str(),
            maximum
        );
    }

    #[test]
    fn reports_deterministic_non_sensitive_failures() {
        let cases = [
            ("", InvalidRequestId::Empty),
            (
                &"secret".repeat(22),
                InvalidRequestId::TooLong { length: 132 },
            ),
            (
                "safe/secret",
                InvalidRequestId::InvalidCharacter { index: 4 },
            ),
            (
                "safeümlaut",
                InvalidRequestId::InvalidCharacter { index: 4 },
            ),
        ];

        for (value, expected) in cases {
            let error = RequestId::parse(value).expect_err("invalid request ID");
            assert_eq!(error, expected);
            if !value.is_empty() {
                assert!(!error.to_string().contains(value));
            }
        }
    }

    #[test]
    fn checks_length_before_character_content() {
        let value = format!("/{}", "a".repeat(128));
        assert_eq!(
            RequestId::parse(&value),
            Err(InvalidRequestId::TooLong { length: 129 })
        );
    }

    #[test]
    fn conversion_forms_enforce_identical_invalid_boundaries() {
        let oversized = "a".repeat(RequestId::MAX_LEN + 1);
        let expected = InvalidRequestId::TooLong { length: 129 };
        assert_eq!(RequestId::from_str(&oversized), Err(expected));
        assert_eq!(RequestId::try_from(oversized.as_str()), Err(expected));
        assert_eq!(RequestId::try_from(oversized), Err(expected));
    }

    #[test]
    fn validation_errors_have_stable_redacted_messages() {
        assert_eq!(
            InvalidRequestId::Empty.to_string(),
            "request ID must not be empty"
        );
        assert_eq!(
            InvalidRequestId::TooLong { length: 129 }.to_string(),
            "request ID length 129 exceeds 128 bytes"
        );
        assert_eq!(
            InvalidRequestId::InvalidCharacter { index: 7 }.to_string(),
            "request ID contains an invalid character at byte index 7"
        );
    }
}
