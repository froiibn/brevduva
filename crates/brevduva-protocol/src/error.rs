//! 파싱·검증 실패 타입.
//!
//! 원칙(PROTOCOL.md 8장): 에러 메시지는 에이전트(LLM)가 읽고 스스로 정정할 수 있게
//! 서술적으로 쓴다 — Display 문구에 "무엇이 왜 틀렸고 어떤 형식이어야 하는지"를 담는다.

use std::fmt;

/// 프로토콜 값 파싱·검증 실패.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// 식별자(org/channel/agent) 규칙 위반 — PROTOCOL.md 2.1.
    InvalidIdent { input: String, reason: &'static str },
    /// 토픽 path/필터 규칙 위반 — PROTOCOL.md 2.3.
    InvalidTopic { input: String, reason: &'static str },
    /// 주소(`agent:`/`topic:`/`broadcast`) 형식 위반 — PROTOCOL.md 2.3.
    InvalidAddress { input: String },
    /// ULID 형식 위반 (id·client_key) — PROTOCOL.md 3장.
    InvalidUlid { input: String },
    /// RFC 3339 타임스탬프 형식 위반 (ts) — PROTOCOL.md 3장.
    InvalidTimestamp { input: String },
    /// 엔벨로프 구조 규칙 위반 (kind별 correlation_id 요구 등) — PROTOCOL.md 3.1.
    InvalidEnvelope { reason: String },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdent { input, reason } => write!(
                f,
                "invalid identifier {input:?}: {reason}. expected lowercase kebab-case \
                 `[a-z0-9-]{{1,64}}`; a leading `_` is reserved for system identities"
            ),
            Self::InvalidTopic { input, reason } => write!(
                f,
                "invalid topic {input:?}: {reason}. expected dot-joined `[a-z0-9-]` segments \
                 like `api-changes.auth`; wildcards `*` (one segment) and `>` (rest, last only) \
                 are allowed in subscription filters only"
            ),
            Self::InvalidAddress { input } => write!(
                f,
                "invalid address {input:?}: expected `agent:{{name}}`, `topic:{{path}}`, \
                 or `broadcast`"
            ),
            Self::InvalidUlid { input } => write!(
                f,
                "invalid ULID {input:?}: expected 26 Crockford base32 characters"
            ),
            Self::InvalidTimestamp { input } => write!(
                f,
                "invalid timestamp {input:?}: expected RFC 3339, e.g. `2026-08-25T09:30:00.000Z`"
            ),
            Self::InvalidEnvelope { reason } => write!(f, "invalid envelope: {reason}"),
        }
    }
}

impl std::error::Error for ParseError {}
