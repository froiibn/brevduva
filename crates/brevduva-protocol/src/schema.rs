//! JSON Schema 생성 — 스펙 정식화의 산출물 (PROTOCOL.md 15장 "스키마 정식화").
//!
//! 리포 루트 `schemas/`의 파일들이 이 함수의 출력 스냅샷이다. 스키마 변경은
//! 반드시 타입 변경 → 스냅샷 갱신(의도적 커밋) 순서로 일어난다 —
//! tests/schema_snapshot.rs가 어긋남을 잡는다.

use schemars::schema_for;
use serde_json::Value;

use crate::capability::Capabilities;
use crate::envelope::Envelope;
use crate::frame::{ClientFrame, ServerFrame};

/// 정식화 대상 루트 스키마 목록: (파일 이름, 스키마 JSON).
///
/// 하위 타입(Address, Ident 등)은 각 루트의 `$defs`로 포함된다 — 루트 넷이 곧
/// 와이어에 실제로 나타나는 문서 전부다 (엔벨로프, 양방향 프레임, 능력 선언).
pub fn root_schemas() -> Vec<(&'static str, Value)> {
    vec![
        (
            "envelope",
            serde_json::to_value(schema_for!(Envelope)).expect("schema serializes"),
        ),
        (
            "client-frame",
            serde_json::to_value(schema_for!(ClientFrame)).expect("schema serializes"),
        ),
        (
            "server-frame",
            serde_json::to_value(schema_for!(ServerFrame)).expect("schema serializes"),
        ),
        (
            "capabilities",
            serde_json::to_value(schema_for!(Capabilities)).expect("schema serializes"),
        ),
    ]
}
