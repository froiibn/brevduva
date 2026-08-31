// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 컨트롤 프레임 — WebSocket 위의 모든 통신 단위 (PROTOCOL.md 7장).
//!
//! 형태: `{ "op": "...", "seq"?: n, "re"?: n, "body"?: {...} }`
//! - `seq`: 발신 측 단조 증가 번호. 응답 프레임은 `re`로 대응 (멀티플렉싱)
//! - 엔벨로프(3장)는 PUB/DELIVER의 body에 실린다
//! - HTTP long-poll 폴백도 같은 프레임을 운반 — 시맨틱 동일 (5.2)
//!
//! 필드 수준 정의는 이 타입들에서 생성되는 schemas/가 정식(normative)이다 —
//! 스펙 산문에 없는 세부(FETCH 커서 등)는 여기가 진실.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::capability::Capabilities;
use crate::envelope::Envelope;
use crate::errcode::ErrorCode;
use crate::ident::Ident;
use crate::ids::{MessageId, Timestamp};
use crate::topic::TopicFilter;

/// 클라이언트 → 서버 프레임.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClientFrame {
    /// 발신 측 단조 증가 번호. ACK처럼 응답 성격의 프레임은 생략 가능.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// 서버 프레임(DELIVER 등)에 대한 응답일 때 그 seq.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub re: Option<u64>,
    #[serde(flatten)]
    pub op: ClientOp,
}

/// 클라이언트 조작 (PROTOCOL.md 5.2 표 + 7장 ACK).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", content = "body")]
pub enum ClientOp {
    /// 토큰 + 능력 선언으로 채널 입장. 멱등 — 재연결 시 같은 토큰으로 다시 JOIN (13.2).
    #[serde(rename = "JOIN")]
    Join {
        channel: Ident,
        token: String,
        capabilities: Capabilities,
    },
    /// 채널 이탈.
    #[serde(rename = "LEAVE")]
    Leave,
    /// 토픽 구독 (inbox·broadcast는 자동 구독).
    #[serde(rename = "SUB")]
    Sub { topics: Vec<TopicFilter> },
    /// 토픽 구독 해지.
    #[serde(rename = "UNSUB")]
    Unsub { topics: Vec<TopicFilter> },
    /// 메시지 발행 — body는 엔벨로프 (id·ts는 서버가 채움).
    #[serde(rename = "PUB")]
    Pub(Envelope),
    /// DELIVER 수신 확인 (`re`로 대상 지정) — at-least-once의 클라이언트 절반.
    #[serde(rename = "ACK")]
    Ack,
    /// 히스토리 조회 — 시간·ID 커서 기반, 페이지 최대 100건 (12.2).
    #[serde(rename = "FETCH")]
    Fetch {
        /// 범위 필터. 생략 시 수신 가능한 전 범위 (inbox·broadcast 포함).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topics: Option<Vec<TopicFilter>>,
        /// 이 ID 이후부터 (ULID 시간 정렬 활용).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_id: Option<MessageId>,
        /// 이 시각 이후부터. after_id와 동시 지정 시 after_id 우선.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_ts: Option<Timestamp>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// 채널 참가자 프레즌스 조회 (5.3) — 응답은 OK body의 `presence`.
    #[serde(rename = "PRESENCE")]
    Presence,
    /// 하트비트 (push 모드 프레즌스 판정, 13.1).
    #[serde(rename = "PING")]
    Ping,
    #[serde(rename = "PONG")]
    Pong,
}

/// 서버 → 클라이언트 프레임.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ServerFrame {
    /// 서버 발신 프레임(DELIVER 등)의 단조 증가 번호.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// 클라이언트 프레임에 대한 응답일 때 그 seq.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub re: Option<u64>,
    #[serde(flatten)]
    pub op: ServerOp,
}

/// 서버 조작.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", content = "body")]
pub enum ServerOp {
    /// 성공 응답. body 형태는 원 조작이 결정한다 (PUB → id, PRESENCE → presence).
    #[serde(rename = "OK")]
    Ok(OkBody),
    /// 실패 응답.
    #[serde(rename = "ERR")]
    Err(ErrBody),
    /// 메시지 전달 — 클라이언트는 `{op:"ACK", re:seq}`로 확인 (at-least-once).
    /// Box: 엔벨로프가 다른 변형 대비 커서 enum 크기 비대를 막는다 (와이어 표현 동일).
    #[serde(rename = "DELIVER")]
    Deliver(Box<Envelope>),
    #[serde(rename = "PING")]
    Ping,
    #[serde(rename = "PONG")]
    Pong,
}

/// OK body — 조작별 결과 필드의 합집합 (없는 필드는 생략).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct OkBody {
    /// PUB 성공 시 발급된 메시지 ID. client_key 중복이면 기존 ID (멱등 성공, 13.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<MessageId>,
    /// PRESENCE 조회 결과.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence: Option<Vec<PresenceEntry>>,
    /// FETCH 결과 페이지.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<Envelope>>,
    /// 전방 호환 — 알 수 없는 결과 필드는 보존.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// ERR body. 원칙(8장): message는 에이전트(LLM)가 읽고 스스로 정정할 수 있게 서술적으로.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ErrBody {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    /// rate/limited일 때 재시도 대기 시간 (12.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

/// 프레즌스 항목 (5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceEntry {
    pub agent: Ident,
    pub state: PresenceState,
    /// 마지막 수신 확인 시각 (idle/offline 판단 참고용).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<Timestamp>,
}

/// 수신 상태 (5.3 표).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PresenceState {
    /// 상시 수신 (데몬, push).
    Online,
    /// 일시 수신 (GUI long-poll 홀드 중).
    Waiting,
    /// 접속 이력은 있으나 현재 안 들음 — 메시지는 큐잉(TTL까지).
    Idle,
    /// 채널 이탈.
    Offline,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PROTOCOL.md 7장 예시 프레임들이 이 타입으로 표현되는지 — 스펙·코드 1:1 회귀 지점.
    #[test]
    fn spec_example_frames_parse() {
        let sub: ClientFrame = serde_json::from_str(
            r#"{ "op": "SUB", "seq": 2, "body": { "topics": ["api-changes.>"] } }"#,
        )
        .unwrap();
        assert_eq!(sub.seq, Some(2));
        assert!(matches!(sub.op, ClientOp::Sub { .. }));

        let ack: ClientFrame = serde_json::from_str(r#"{ "op": "ACK", "re": 101 }"#).unwrap();
        assert_eq!(ack.re, Some(101));
        assert!(matches!(ack.op, ClientOp::Ack));

        let err: ServerFrame = serde_json::from_str(
            r#"{ "op": "ERR", "re": 2, "body": { "code": "channel/no-grant", "message": "no grant", "retryable": false } }"#,
        )
        .unwrap();
        match &err.op {
            ServerOp::Err(body) => {
                assert_eq!(body.code, ErrorCode::ChannelNoGrant);
                assert!(!body.retryable);
            }
            other => panic!("expected ERR, got {other:?}"),
        }

        let ok: ServerFrame = serde_json::from_str(
            r#"{ "op": "OK", "re": 3, "body": { "id": "01ARZ3NDEKTSV4RRFFQ69G5FAV" } }"#,
        )
        .unwrap();
        assert!(matches!(ok.op, ServerOp::Ok(_)));
    }

    #[test]
    fn bodyless_ops_round_trip() {
        let ping = ClientFrame {
            seq: Some(9),
            re: None,
            op: ClientOp::Ping,
        };
        let json = serde_json::to_string(&ping).unwrap();
        let back: ClientFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(ping, back);
    }
}
