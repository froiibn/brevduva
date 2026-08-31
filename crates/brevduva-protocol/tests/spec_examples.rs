// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 스펙 예시 동형 테스트 — PROTOCOL.md의 예시 구조가 crate 타입으로 그대로
//! 표현되는지 검증하는 회귀 지점. 스펙 예시의 `01J5X...` 류 자리표시자는
//! 유효한 ULID로 치환했다 (구조는 동일).

use brevduva_protocol::{
    Address, ClientFrame, ClientOp, Envelope, Expects, Kind, ServerFrame, ServerOp,
};

/// PROTOCOL.md 3장 엔벨로프 예시 (전달된 형태 — id·ts 포함).
#[test]
fn section3_envelope_example() {
    let json = r#"{
        "v": 1,
        "id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "ts": "2026-08-25T09:30:00.000Z",
        "client_key": "01BX5ZZKBKACTAV9WEVGEMMVRZ",
        "from": "backend",
        "to": "agent:frontend",
        "kind": "request",
        "correlation_id": null,
        "expects": "reply",
        "ttl_ms": 600000,
        "hops": 0,
        "content_type": "text/markdown",
        "payload": "로그인 API 응답 포맷이 이렇게 바뀐다...",
        "payload_ref": null,
        "meta": {}
    }"#;
    let envelope: Envelope = serde_json::from_str(json).unwrap();
    envelope.validate_delivered().unwrap();
    assert_eq!(envelope.kind, Kind::Request);
    assert_eq!(envelope.expects, Some(Expects::Reply));
    assert_eq!(envelope.to, Address::parse("agent:frontend").unwrap());

    // 재직렬화 후에도 같은 의미로 읽히는지 (null 필드는 생략 표기로 정규화된다)
    let round: Envelope = serde_json::from_str(&serde_json::to_string(&envelope).unwrap()).unwrap();
    assert_eq!(envelope, round);
}

/// PROTOCOL.md 7장 프레임 예시 — 클라이언트 JOIN/SUB/PUB, 서버 OK/ERR/DELIVER.
#[test]
fn section7_frame_examples() {
    let join: ClientFrame = serde_json::from_str(
        r#"{ "op": "JOIN", "seq": 1, "body": {
            "channel": "myapp",
            "token": "tok-123",
            "capabilities": {
                "agent": "frontend",
                "description": "React 앱 담당",
                "max_inline_bytes": 262144,
                "content_types": ["text/*"],
                "encodings": ["json"],
                "modes": ["poll"],
                "meta": {}
            }
        } }"#,
    )
    .unwrap();
    assert!(matches!(join.op, ClientOp::Join { .. }));

    let publish: ClientFrame = serde_json::from_str(
        r#"{ "op": "PUB", "seq": 3, "body": {
            "v": 1,
            "client_key": "01BX5ZZKBKACTAV9WEVGEMMVRZ",
            "from": "backend",
            "to": "broadcast",
            "kind": "message",
            "expects": "ack",
            "hops": 0,
            "content_type": "text/markdown",
            "payload": "백엔드가 이렇게 바뀌었다 — 영향받는 애들은 맞춰라",
            "meta": {}
        } }"#,
    )
    .unwrap();
    match &publish.op {
        ClientOp::Pub(envelope) => {
            envelope.validate().unwrap();
            // 발행 시점 — 서버 발급 필드는 아직 없다
            assert!(envelope.id.is_none() && envelope.ts.is_none());
        }
        other => panic!("expected PUB, got {other:?}"),
    }

    let deliver: ServerFrame = serde_json::from_str(
        r#"{ "op": "DELIVER", "seq": 101, "body": {
            "v": 1,
            "id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "ts": "2026-08-25T09:30:00.000Z",
            "client_key": "01BX5ZZKBKACTAV9WEVGEMMVRZ",
            "from": "backend",
            "to": "agent:frontend",
            "kind": "request",
            "expects": "reply",
            "hops": 0,
            "content_type": "text/markdown",
            "payload": "스펙 알려줘",
            "meta": {}
        } }"#,
    )
    .unwrap();
    match &deliver.op {
        ServerOp::Deliver(envelope) => envelope.validate_delivered().unwrap(),
        other => panic!("expected DELIVER, got {other:?}"),
    }
}
