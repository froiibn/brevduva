//! 메시지 ID·멱등 키·타임스탬프 (PROTOCOL.md 3장, 13.3).
//!
//! - `id`: 서버 발급 ULID — 시간 정렬 가능, 수신 측 중복 제거의 기준
//! - `client_key`: 클라이언트 발급 ULID — 발행 재시도 멱등성 (서버 10분 창 중복 제거)
//! - `ts`: 서버 수신 시각, RFC 3339 — 진실의 원천은 서버 시계

use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;

use crate::error::ParseError;

/// ULID 문자열 뉴타입. 와이어 표현(26자 Crockford base32)을 그대로 보존한다 —
/// 파싱해 128비트로 들고 다니면 대소문자 정규화로 와이어 바이트가 달라질 수 있다.
macro_rules! ulid_newtype {
    ($ty:ident, $desc:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $ty(String);

        impl $ty {
            pub fn parse(input: &str) -> Result<Self, ParseError> {
                ulid::Ulid::from_string(input)
                    .map(|_| Self(input.to_owned()))
                    .map_err(|_| ParseError::InvalidUlid {
                        input: input.to_owned(),
                    })
            }

            /// 새 ULID 발급 (현재 시각 기반).
            pub fn generate() -> Self {
                Self(ulid::Ulid::new().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl TryFrom<String> for $ty {
            type Error = ParseError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(&value)
            }
        }
        impl From<$ty> for String {
            fn from(value: $ty) -> Self {
                value.0
            }
        }
        impl JsonSchema for $ty {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($ty).into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "string",
                    "description": $desc,
                    "pattern": "^[0-7][0-9A-HJKMNP-TV-Z]{25}$"
                })
            }
        }
    };
}

ulid_newtype!(
    MessageId,
    "server-issued ULID: time-sortable unique message id (PROTOCOL.md 3)"
);
ulid_newtype!(
    ClientKey,
    "client-issued ULID: publish retry idempotency key, deduplicated by the server within a 10-minute window (PROTOCOL.md 13.3)"
);

/// RFC 3339 타임스탬프 뉴타입. 시각 연산은 소비자 소관 — 여기서는 형식 검증과
/// 와이어 문자열 보존만 책임진다.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Timestamp(String);

impl Timestamp {
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        time::OffsetDateTime::parse(input, &Rfc3339)
            .map(|_| Self(input.to_owned()))
            .map_err(|_| ParseError::InvalidTimestamp {
                input: input.to_owned(),
            })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl TryFrom<String> for Timestamp {
    type Error = ParseError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}
impl From<Timestamp> for String {
    fn from(value: Timestamp) -> Self {
        value.0
    }
}
impl JsonSchema for Timestamp {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Timestamp".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "format": "date-time",
            "description": "RFC 3339 timestamp; source of truth is the server clock (PROTOCOL.md 3)"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulid_parse_and_generate() {
        let generated = MessageId::generate();
        assert!(MessageId::parse(generated.as_str()).is_ok());
        assert!(MessageId::parse("not-a-ulid").is_err());
        assert!(ClientKey::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").is_ok());
    }

    #[test]
    fn timestamp_validation() {
        assert!(Timestamp::parse("2026-08-25T09:30:00.000Z").is_ok());
        assert!(Timestamp::parse("2026-08-25 09:30:00").is_err());
        assert!(Timestamp::parse("").is_err());
    }
}
