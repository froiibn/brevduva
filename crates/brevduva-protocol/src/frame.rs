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
    /// HTTP 전달 증명과 re를 함께 검증하는 수신 확인.
    #[serde(rename = "ACK_DELIVERY")]
    AckDelivery { delivery_id: String },
    /// 미확인 전달의 착수 동안 수신 점유 인계를 유보한다 (7.1).
    #[serde(rename = "RESERVE")]
    Reserve { deliveries: Vec<u64> },
    /// 착수 예약 해제. 미확인 전달을 소비하지 않는다.
    #[serde(rename = "RELEASE")]
    Release,
    /// 미확인 전달을 에이전트에게 넘기는 중 — 재전송 대기를 연장한다 (7.2, 2026-09-11).
    /// 수신 확인(ACK)은 에이전트가 받았다는 증거가 있을 때만 보내므로, 넘기는 동안의 대기를
    /// 정직하게 표현한다. 연장은 첫 WORKING부터 `DeliveryTerms::working_max_ms`까지.
    #[serde(rename = "WORKING")]
    Working { deliveries: Vec<u64> },
    /// 넘길 곳이 없거나 넘겨도 되는지 판단할 수 없어 큐로 되돌린다 (7.2, 2026-09-11).
    /// `delay_ms`(서버 범위로 잘림) 뒤 다시 전달되며, 처리 실패가 아니므로 격리 판정에서 뺀다.
    #[serde(rename = "DEFER")]
    Defer { deliveries: Vec<u64>, delay_ms: u64 },
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
        /// 이 ID **이전**까지 — 역순 페이지(`newest_first`)의 커서 (2026-09-05).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_id: Option<MessageId>,
        /// true면 **최신부터 역순**으로 `limit`개 (2026-09-05). 종전엔 과거→현재 한 방향뿐이라
        /// "최근 무슨 일이 있었나"를 보려면 처음부터 끝까지 넘겨야 했다. 다음 페이지(더 과거)는
        /// 마지막으로 받은 id를 `before_id`로. `after_id`/`after_ts`와 함께 쓰지 않는다.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        newest_first: bool,
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
    /// JOIN 성공 시 전달 연장·연기 조건 (7.2) — 이 필드가 있는 서버만 `WORKING`·`DEFER`를 받는다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<DeliveryTerms>,
    /// 전방 호환 — 알 수 없는 결과 필드는 보존.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 전달 연장·연기 조건 (PROTOCOL.md 7.2·12.2) — JOIN `OK`에 실린다. 수치는 서버 설정이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryTerms {
    /// 확인·연장 없는 전달을 다시 전달하기까지의 대기. `WORKING`은 이 안에 반복한다.
    pub ack_wait_ms: u64,
    /// 한 전달의 `WORKING` 연장 상한 — 첫 연장부터 센다.
    pub working_max_ms: u64,
    /// `DEFER`의 `delay_ms` 허용 범위.
    pub defer_min_ms: u64,
    pub defer_max_ms: u64,
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

    /// PROTOCOL.md 7.2 예시 프레임과 JOIN OK의 조건 표시 (2026-09-11).
    #[test]
    fn delivery_extension_frames_parse() {
        let working: ClientFrame = serde_json::from_str(
            r#"{ "op": "WORKING", "seq": 57, "body": { "deliveries": [101] } }"#,
        )
        .unwrap();
        assert_eq!(
            working.op,
            ClientOp::Working {
                deliveries: vec![101]
            }
        );
        let defer: ClientFrame = serde_json::from_str(
            r#"{ "op": "DEFER", "seq": 58, "body": { "deliveries": [102], "delay_ms": 60000 } }"#,
        )
        .unwrap();
        assert_eq!(
            defer.op,
            ClientOp::Defer {
                deliveries: vec![102],
                delay_ms: 60000
            }
        );

        let joined: ServerFrame = serde_json::from_str(
            r#"{ "op": "OK", "re": 1, "body": { "receiver": {}, "delivery": { "ack_wait_ms": 30000, "working_max_ms": 3600000, "defer_min_ms": 5000, "defer_max_ms": 600000 } } }"#,
        )
        .unwrap();
        match joined.op {
            ServerOp::Ok(body) => {
                assert_eq!(
                    body.delivery,
                    Some(DeliveryTerms {
                        ack_wait_ms: 30000,
                        working_max_ms: 3_600_000,
                        defer_min_ms: 5000,
                        defer_max_ms: 600_000,
                    })
                );
                assert!(body.extra.contains_key("receiver"), "다른 결과 필드는 보존");
            }
            other => panic!("expected OK, got {other:?}"),
        }
        // 두 프레임을 모르는 서버의 JOIN OK — 클라이언트는 보내지 않는다
        let old: ServerFrame =
            serde_json::from_str(r#"{ "op": "OK", "re": 1, "body": {} }"#).unwrap();
        match old.op {
            ServerOp::Ok(body) => assert_eq!(body.delivery, None),
            other => panic!("expected OK, got {other:?}"),
        }
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
