//! 식별자 — org / channel / agent 이름 (PROTOCOL.md 2.1).
//!
//! 규칙: `[a-z0-9-]{1,64}` 소문자 케밥 케이스. `_` 접두는 시스템 예약
//! (예: `_system`, `_dashboard`) — 와이어에서는 유효하지만 일반 등록은 불가.

use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use crate::error::ParseError;

/// 식별자 최대 길이 (시스템 예약 `_` 접두 제외한 본문 기준).
pub const IDENT_MAX_LEN: usize = 64;

/// org / channel / agent 공용 식별자.
///
/// 하나의 타입으로 둔 이유: 세 네임스페이스의 문법이 동일하며(2.1), 타입을 나누면
/// 조합 지점(주소, 프레임)마다 변환 소음만 생긴다. 의미 구분은 필드명이 담당한다.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Ident(String);

impl Ident {
    /// 와이어에서 유효한 식별자를 파싱한다. 시스템 예약(`_` 접두)도 통과한다 —
    /// 수신 경로에서 `_system` 발신 이벤트를 표현해야 하기 때문.
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        let body = input.strip_prefix('_').unwrap_or(input);
        if body.is_empty() {
            return Err(ParseError::InvalidIdent {
                input: input.to_owned(),
                reason: "empty",
            });
        }
        if body.len() > IDENT_MAX_LEN {
            return Err(ParseError::InvalidIdent {
                input: input.to_owned(),
                reason: "longer than 64 characters",
            });
        }
        if !body
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(ParseError::InvalidIdent {
                input: input.to_owned(),
                reason: "contains characters outside [a-z0-9-]",
            });
        }
        Ok(Self(input.to_owned()))
    }

    /// 시스템 예약 식별자인가 (`_` 접두).
    pub fn is_system(&self) -> bool {
        self.0.starts_with('_')
    }

    /// 사용자 등록 가능 여부 검증 — 시스템 예약을 거부한다 (등록 API 경로용).
    pub fn parse_registrable(input: &str) -> Result<Self, ParseError> {
        let ident = Self::parse(input)?;
        if ident.is_system() {
            return Err(ParseError::InvalidIdent {
                input: input.to_owned(),
                reason: "leading `_` is reserved for system identities",
            });
        }
        Ok(ident)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for Ident {
    type Error = ParseError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<Ident> for String {
    fn from(value: Ident) -> Self {
        value.0
    }
}

impl JsonSchema for Ident {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Ident".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "org/channel/agent identifier: lowercase kebab-case, max 64 chars; leading `_` reserved for system identities (PROTOCOL.md 2.1)",
            "pattern": "^_?[a-z0-9-]{1,64}$"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_idents() {
        for ok in ["frontend", "a", "my-agent-01", "_system", &"x".repeat(64)] {
            assert!(Ident::parse(ok).is_ok(), "should accept {ok:?}");
        }
    }

    #[test]
    fn rejects_invalid_idents() {
        for bad in [
            "",
            "_",
            "Frontend",
            "front_end",
            "한글",
            "a.b",
            &"x".repeat(65),
        ] {
            assert!(Ident::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn registrable_rejects_system_prefix() {
        assert!(Ident::parse_registrable("frontend").is_ok());
        assert!(Ident::parse_registrable("_system").is_err());
    }

    #[test]
    fn serde_round_trip() {
        let id: Ident = serde_json::from_str("\"frontend\"").unwrap();
        assert_eq!(id.as_str(), "frontend");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"frontend\"");
        assert!(serde_json::from_str::<Ident>("\"BAD\"").is_err());
    }
}
