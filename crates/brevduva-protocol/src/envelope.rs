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

/// report 본문이 **진행 알림**인가 (3.1, 2026-09-04 · 2026-09-05 확장).
///
/// 진행 알림 = JSON `status`가 `in-progress`이거나, **본문이 JSON이 아니거나 `status`가 없는 것**.
/// 최종 응답은 `reply`, 또는 `status`가 `in-progress`가 아닌 JSON report(`failed` 등)뿐이다.
/// 확장의 발단(2026-09-05 실측): 한 세션이 마크다운 본문(`## in-progress …`)의 report로 착수를
/// 알렸고, 기다리는 쪽(원격 `wait_for_reply`)이 "JSON이 아니다 = in-progress가 아니다 = 최종"으로
/// 읽어 `replied`를 돌렸다. 어휘를 못 밝힌 보고를 답으로 단정하는 것이 오독의 뿌리다 — 모르면
/// 진행으로 본다. 어댑터 셋(리시버 데몬·로컬 MCP·원격 MCP)이 같은 판정을 쓰도록 여기 둔다.
pub fn report_payload_is_progress(payload: Option<&str>) -> bool {
    let Some(payload) = payload else {
        return true;
    };
    match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(v) => match v.get("status").and_then(serde_json::Value::as_str) {
            Some(status) => status == "in-progress",
            None => true,
        },
        Err(_) => true,
    }
}

/// 어댑터의 `report` 도구가 본문을 발행 전에 어휘에 맞춘다 (3.1): JSON이 아니거나 `status`가 없는
/// 본문은 `{"status":"in-progress","note":본문}`으로 감싼다. 감쌀 필요가 없으면 None.
/// 읽는 쪽 규칙(위)과 뜻은 같고, 옛 읽는 쪽(감싸기 이전 버전)도 올바로 읽게 하는 것이 목적이다.
pub fn coerce_report_payload(payload: &str) -> Option<String> {
    if !report_payload_is_progress(Some(payload)) {
        return None;
    }
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(payload)
        && map.get("status").is_some_and(|s| s == "in-progress")
    {
        return None; // 이미 어휘에 맞는 진행 알림
    }
    Some(serde_json::json!({ "status": "in-progress", "note": payload }).to_string())
}

impl Envelope {
    /// 진행 알림인가 — `report`이면서 본문이 [`report_payload_is_progress`] (3.1). 응답을 기다리는 쪽은
    /// 이것을 **최종 답으로 세지 않는다** — 진행 정보로 넘기고 계속 기다린다 (9장 6항).
    pub fn is_progress_report(&self) -> bool {
        self.kind == Kind::Report && report_payload_is_progress(self.payload.as_deref())
    }

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

    /// 3.1 진행 알림 판정 (2026-09-05 확장): JSON in-progress·JSON status 없음·JSON 아님·본문 없음은
    /// 전부 진행 알림, `failed`나 다른 status의 JSON만 최종. report가 아니면 진행 알림이 아니다.
    #[test]
    fn progress_report_vocabulary() {
        assert!(report_payload_is_progress(Some(
            r#"{"status":"in-progress","note":"x"}"#
        )));
        assert!(report_payload_is_progress(Some(
            r#"{"note":"no status here"}"#
        )));
        assert!(report_payload_is_progress(Some(
            "## in-progress\n마크다운 착수 알림"
        )));
        assert!(report_payload_is_progress(None));
        assert!(!report_payload_is_progress(Some(
            r#"{"status":"failed","reason":"boom"}"#
        )));
        assert!(!report_payload_is_progress(Some(
            r#"{"status":"done","result":1}"#
        )));

        let mut e = base();
        e.kind = Kind::Report;
        e.payload = Some("## in-progress".to_owned());
        assert!(e.is_progress_report());
        e.payload = Some(r#"{"status":"failed","reason":"x"}"#.to_owned());
        assert!(!e.is_progress_report());
        e.kind = Kind::Reply;
        e.payload = Some("plain reply".to_owned());
        assert!(
            !e.is_progress_report(),
            "only reports can be progress notices"
        );
    }

    /// 어댑터 `report` 도구의 본문 정규화: JSON 아님·status 없음 → in-progress로 감쌈(본문은 note에 보존),
    /// 이미 어휘에 맞는 것(in-progress / failed / 다른 status)은 그대로.
    #[test]
    fn coerce_report_payload_wraps_only_non_vocabulary_bodies() {
        let wrapped = coerce_report_payload("## in-progress\n착수").unwrap();
        let v: Value = serde_json::from_str(&wrapped).unwrap();
        assert_eq!(v["status"], "in-progress");
        assert_eq!(v["note"], "## in-progress\n착수");
        let wrapped = coerce_report_payload(r#"{"note":"no status"}"#).unwrap();
        let v: Value = serde_json::from_str(&wrapped).unwrap();
        assert_eq!(v["status"], "in-progress");
        assert!(coerce_report_payload(r#"{"status":"in-progress","note":"x"}"#).is_none());
        assert!(coerce_report_payload(r#"{"status":"failed","reason":"x"}"#).is_none());
        assert!(coerce_report_payload(r#"{"status":"done"}"#).is_none());
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
