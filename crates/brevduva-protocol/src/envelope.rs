// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 메시지 엔벨로프 — 모든 메시지의 공통 스키마 (PROTOCOL.md 3장).
//!
//! 직렬화는 v1에서 JSON(UTF-8). `id`·`ts`는 서버가 채우므로 클라이언트 발행 시점에는
//! 없다 — Option으로 모델링하고, 전달(DELIVER)된 엔벨로프는 `validate_delivered`로
//! 존재를 보장한다. 로봇 단계의 CBOR 협상은 능력 선언 `encodings`에 자리만 확보.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::PROTOCOL_VERSION;
use crate::address::Address;
use crate::error::ParseError;
use crate::ident::Ident;
use crate::ids::{ClientKey, MessageId, Timestamp};

/// 메시지 종류 (PROTOCOL.md 3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// 단방향 알림/정보.
    Message,
    /// 응답을 기대하는 요청 (지시 포함).
    Request,
    /// request에 대한 응답 — correlation_id 필수.
    Reply,
    /// 수신 확인 ("받았고 처리하겠다/무관하다") — correlation_id 필수.
    Ack,
    /// 작업 완료/실패 보고 — correlation_id 필수 (원본 request/message).
    Report,
    /// 시스템 이벤트 (입장/퇴장/능력 변경 등, `_system` 발신).
    Event,
}

impl Kind {
    /// 이 kind가 원본 메시지를 가리키는 `correlation_id`를 요구하는가 (3.1 표).
    pub fn requires_correlation(self) -> bool {
        matches!(self, Self::Reply | Self::Ack | Self::Report)
    }
}

/// 발신자가 기대하는 반응 (PROTOCOL.md 3장 `expects`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Expects {
    Ack,
    Reply,
}

/// claim-check 참조 — 인라인 임계값(256KB) 초과 페이로드 (PROTOCOL.md 3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PayloadRef {
    /// 서버 발급 blob ID.
    pub id: String,
    /// 바이트 크기.
    pub size: u64,
    /// 무결성 해시 (hex).
    pub sha256: String,
    /// 페이로드 MIME 타입.
    pub content_type: String,
}

/// 메시지 엔벨로프 (PROTOCOL.md 3장).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Envelope {
    /// 프로토콜 버전 — 현재 1.
    pub v: u8,
    /// 서버 발급 ULID. 클라이언트 PUB 시점에는 없다 (서버가 채움).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<MessageId>,
    /// 서버 수신 시각. 클라이언트 PUB 시점에는 없다 (진실의 원천은 서버 시계).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<Timestamp>,
    /// 클라이언트 발급 멱등 키 — 발행 재시도용 (PROTOCOL.md 13.3). PUB에 필수.
    pub client_key: ClientKey,
    /// 발신 에이전트.
    pub from: Ident,
    /// 주소 (2.3).
    pub to: Address,
    /// 메시지 종류 (3.1).
    pub kind: Kind,
    /// reply/ack/report가 원본을 가리킬 때 — 해당 kind에서 필수 (3.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<MessageId>,
    /// 발신자가 기대하는 반응.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expects: Option<Expects>,
    /// 만료(ms). 생략 시 채널 `default_ttl_ms` 적용 (12.1) — 만료 후 미전달분은 폐기.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
    /// 에이전트 간 연쇄 전파 깊이 — 폭주 방지 (3.3). 원본에 반응한 메시지는 +1.
    #[serde(default)]
    pub hops: u32,
    /// 페이로드 MIME 타입.
    pub content_type: String,
    /// 인라인 페이로드 (임계값 이하). JSON 페이로드도 직렬화된 문자열로 싣는다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
    /// claim-check 참조 (임계값 초과 시, 3.2). payload와 동시 설정 불가.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_ref: Option<PayloadRef>,
    /// 확장 슬롯: 서명, 트레이스, parent_id 등. 알 수 없는 키는 무시하고 보존한다.
    #[serde(default)]
    pub meta: Map<String, Value>,
}

impl Envelope {
    /// 구조 규칙 검증 — 발행·전달 공통 (3장).
    ///
    /// 크기 임계값·hops 상한·능력 검증 같은 정책 집행은 서버 소관 (수치는 12장 설정).
    /// 여기서는 스키마 차원의 불변식만 본다.
    pub fn validate(&self) -> Result<(), ParseError> {
        if self.v != PROTOCOL_VERSION {
            return Err(ParseError::InvalidEnvelope {
                reason: format!(
                    "unsupported protocol version {} (expected {PROTOCOL_VERSION})",
                    self.v
                ),
            });
        }
        if self.kind.requires_correlation() && self.correlation_id.is_none() {
            return Err(ParseError::InvalidEnvelope {
                reason: format!(
                    "kind {:?} requires correlation_id pointing at the original message (PROTOCOL.md 3.1)",
                    self.kind
                ),
            });
        }
        if self.payload.is_some() && self.payload_ref.is_some() {
            return Err(ParseError::InvalidEnvelope {
                reason: "payload and payload_ref are mutually exclusive: inline up to the \
                         threshold, claim-check reference above it (PROTOCOL.md 3.2)"
                    .to_owned(),
            });
        }
        Ok(())
    }

    /// 전달된(DELIVER) 엔벨로프 검증 — 서버 발급 필드의 존재까지 보장.
    pub fn validate_delivered(&self) -> Result<(), ParseError> {
        self.validate()?;
        if self.id.is_none() || self.ts.is_none() {
            return Err(ParseError::InvalidEnvelope {
                reason: "delivered envelope must carry server-issued id and ts".to_owned(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Envelope {
        Envelope {
            v: 1,
            id: None,
            ts: None,
            client_key: ClientKey::generate(),
            from: Ident::parse("backend").unwrap(),
            to: Address::parse("agent:frontend").unwrap(),
            kind: Kind::Request,
            correlation_id: None,
            expects: Some(Expects::Reply),
            ttl_ms: Some(600_000),
            hops: 0,
            content_type: "text/markdown".to_owned(),
            payload: Some("로그인 API 응답 포맷이 바뀌었다".to_owned()),
            payload_ref: None,
            meta: Map::new(),
        }
    }

    #[test]
    fn valid_publish_envelope_passes() {
        base().validate().unwrap();
    }

    #[test]
    fn reply_without_correlation_fails() {
        let mut e = base();
        e.kind = Kind::Reply;
        assert!(e.validate().is_err());
        e.correlation_id = Some(MessageId::generate());
        e.validate().unwrap();
    }

    #[test]
    fn payload_and_ref_are_exclusive() {
        let mut e = base();
        e.payload_ref = Some(PayloadRef {
            id: "blob_x".to_owned(),
            size: 1,
            sha256: "ab".to_owned(),
            content_type: "text/markdown".to_owned(),
        });
        assert!(e.validate().is_err());
    }

    #[test]
    fn delivered_requires_id_and_ts() {
        let mut e = base();
        assert!(e.validate_delivered().is_err());
        e.id = Some(MessageId::generate());
        e.ts = Some(Timestamp::parse("2026-08-25T09:30:00.000Z").unwrap());
        e.validate_delivered().unwrap();
    }

    #[test]
    fn unknown_meta_keys_survive_round_trip() {
        let mut e = base();
        e.meta.insert("x-future".to_owned(), Value::from(42));
        let json = serde_json::to_string(&e).unwrap();
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.meta.get("x-future"), Some(&Value::from(42)));
    }
}
