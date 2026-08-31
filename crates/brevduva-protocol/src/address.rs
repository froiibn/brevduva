// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 메시지 주소 — 엔벨로프의 `to` 필드 (PROTOCOL.md 2.3).
//!
//! 와이어 표현은 문자열: `agent:{name}` | `topic:{path}` | `broadcast`.
//! 내부 라우팅 키는 서버 소관 — 클라이언트가 아는 주소 형식은 이 셋뿐이다 (경계 원칙).

use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use crate::error::ParseError;
use crate::ident::Ident;
use crate::topic::TopicPath;

/// 메시지의 수신 대상.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Address {
    /// 지명 전달 (1:1) — 해당 에이전트의 inbox로.
    Agent(Ident),
    /// 토픽 구독자에게 (1:N, 선택 구독).
    Topic(TopicPath),
    /// 채널 전체 (발신자 제외).
    Broadcast,
}

impl Address {
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        if input == "broadcast" {
            return Ok(Self::Broadcast);
        }
        if let Some(name) = input.strip_prefix("agent:") {
            return Ident::parse(name)
                .map(Self::Agent)
                .map_err(|_| ParseError::InvalidAddress {
                    input: input.to_owned(),
                });
        }
        if let Some(path) = input.strip_prefix("topic:") {
            return TopicPath::parse(path).map(Self::Topic).map_err(|_| {
                ParseError::InvalidAddress {
                    input: input.to_owned(),
                }
            });
        }
        Err(ParseError::InvalidAddress {
            input: input.to_owned(),
        })
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent(name) => write!(f, "agent:{name}"),
            Self::Topic(path) => write!(f, "topic:{path}"),
            Self::Broadcast => f.write_str("broadcast"),
        }
    }
}

impl TryFrom<String> for Address {
    type Error = ParseError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<Address> for String {
    fn from(value: Address) -> Self {
        value.to_string()
    }
}

impl JsonSchema for Address {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Address".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "message destination: `agent:{name}` (1:1 inbox), `topic:{path}` (subscribers), or `broadcast` (whole channel except sender) (PROTOCOL.md 2.3)",
            "pattern": "^(agent:_?[a-z0-9-]{1,64}|topic:[a-z0-9-]+(\\.[a-z0-9-]+)*|broadcast)$"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_three_forms() {
        assert_eq!(
            Address::parse("agent:frontend").unwrap().to_string(),
            "agent:frontend"
        );
        assert_eq!(
            Address::parse("topic:api-changes.auth")
                .unwrap()
                .to_string(),
            "topic:api-changes.auth"
        );
        assert_eq!(Address::parse("broadcast").unwrap(), Address::Broadcast);
    }

    #[test]
    fn rejects_malformed_addresses() {
        for bad in [
            "",
            "frontend",       // 접두 없음
            "agent:",         // 이름 없음
            "agent:Frontend", // 대문자
            "topic:a.*",      // 주소에는 와일드카드 불가 (필터는 SUB 전용)
            "topic:",
            "Broadcast",
            "inbox:frontend", // 내부 라우팅 키 누출 방지 — 주소 문법이 아님
        ] {
            assert!(Address::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn serde_round_trip() {
        let a: Address = serde_json::from_str("\"agent:frontend\"").unwrap();
        assert_eq!(serde_json::to_string(&a).unwrap(), "\"agent:frontend\"");
    }
}
