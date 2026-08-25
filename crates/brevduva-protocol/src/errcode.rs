//! 에러 코드 — `{범주}/{코드}` 문자열 (PROTOCOL.md 8장).
//!
//! 알려진 코드는 enum으로 고정하되, 미래 서버가 새 코드를 보내도 클라이언트가
//! 깨지지 않도록 `Unknown`으로 보존한다 (전방 호환). retryable 플래그는
//! ERR 프레임 body의 별도 필드다 — 코드 자체에 넣지 않는다.

use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

/// 프로토콜 에러 코드.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum ErrorCode {
    /// 토큰 무효/폐기됨.
    AuthInvalidToken,
    /// 채널 참가 권한 없음.
    ChannelNoGrant,
    /// 채널 없음.
    ChannelNotFound,
    /// reject 정책 채널에서 이미 활성 세션 존재.
    AgentSessionConflict,
    /// 인라인 임계값 초과 — payload_ref(BLOB_PUT)를 쓰라는 신호.
    MsgTooLarge,
    /// max_hops 초과 — 연쇄 차단.
    MsgHopsExceeded,
    /// `agent:{name}`이 채널에 없음.
    MsgUnknownRecipient,
    /// 수신자 능력 선언 위반 (크기/타입).
    MsgCapabilityMismatch,
    /// 발행 속도 제한 — ERR body의 retry_after_ms 후 재시도.
    RateLimited,
    /// 서버 오류.
    ServerInternal,
    /// 이 crate가 모르는 코드 — 전방 호환용 보존.
    Unknown(String),
}

impl ErrorCode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::AuthInvalidToken => "auth/invalid-token",
            Self::ChannelNoGrant => "channel/no-grant",
            Self::ChannelNotFound => "channel/not-found",
            Self::AgentSessionConflict => "agent/session-conflict",
            Self::MsgTooLarge => "msg/too-large",
            Self::MsgHopsExceeded => "msg/hops-exceeded",
            Self::MsgUnknownRecipient => "msg/unknown-recipient",
            Self::MsgCapabilityMismatch => "msg/capability-mismatch",
            Self::RateLimited => "rate/limited",
            Self::ServerInternal => "server/internal",
            Self::Unknown(s) => s,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<String> for ErrorCode {
    fn from(value: String) -> Self {
        match value.as_str() {
            "auth/invalid-token" => Self::AuthInvalidToken,
            "channel/no-grant" => Self::ChannelNoGrant,
            "channel/not-found" => Self::ChannelNotFound,
            "agent/session-conflict" => Self::AgentSessionConflict,
            "msg/too-large" => Self::MsgTooLarge,
            "msg/hops-exceeded" => Self::MsgHopsExceeded,
            "msg/unknown-recipient" => Self::MsgUnknownRecipient,
            "msg/capability-mismatch" => Self::MsgCapabilityMismatch,
            "rate/limited" => Self::RateLimited,
            "server/internal" => Self::ServerInternal,
            _ => Self::Unknown(value),
        }
    }
}

impl From<ErrorCode> for String {
    fn from(value: ErrorCode) -> Self {
        value.as_str().to_owned()
    }
}

impl JsonSchema for ErrorCode {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ErrorCode".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "error code as `{category}/{code}` (PROTOCOL.md 8); clients must preserve unknown codes",
            "pattern": "^[a-z]+/[a-z-]+$"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_round_trip() {
        let c: ErrorCode = serde_json::from_str("\"rate/limited\"").unwrap();
        assert_eq!(c, ErrorCode::RateLimited);
        assert_eq!(serde_json::to_string(&c).unwrap(), "\"rate/limited\"");
    }

    #[test]
    fn unknown_codes_are_preserved() {
        let c: ErrorCode = serde_json::from_str("\"future/new-code\"").unwrap();
        assert_eq!(c, ErrorCode::Unknown("future/new-code".to_owned()));
        assert_eq!(serde_json::to_string(&c).unwrap(), "\"future/new-code\"");
    }
}
