// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 능력 선언 (capability advertisement) — PROTOCOL.md 4장.
//!
//! JOIN 시 선언, 변경 시 `event`로 채널 전파. 원칙: 프로토콜은 멍청하게,
//! 적응 부담은 똑똑한(발신) 쪽이 진다 — 발신 어댑터가 수신자 선언을 조회해 맞춘다.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ident::Ident;

/// 수신 방식 (PROTOCOL.md 4장 `modes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReceiveMode {
    /// 상시 수신 (데몬).
    Push,
    /// blocking 도구 재호출 루프 (GUI 어댑터).
    Poll,
}

/// 에이전트 능력 선언.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    /// 선언 주체 에이전트.
    pub agent: Ident,
    /// 동료 에이전트가 읽는 소개 — 브로드캐스트 관련성 판단·지명 대상 선택의 근거.
    pub description: String,
    /// 이 크기까지 한 메시지로 수신 가능 (컨텍스트 예산 반영).
    pub max_inline_bytes: u64,
    /// 수신 가능 콘텐츠 타입 (예: `text/*`, `application/json`).
    pub content_types: Vec<String>,
    /// 직렬화 협상 자리 — v1은 `json`, 로봇 단계에 `cbor` 추가 예정.
    pub encodings: Vec<String>,
    /// 수신 방식.
    pub modes: Vec<ReceiveMode>,
    /// 확장 슬롯.
    #[serde(default)]
    pub meta: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PROTOCOL.md 4장 예시와 동형의 JSON이 파싱되는지 — 스펙·코드 1:1 회귀 지점.
    #[test]
    fn spec_example_shape_parses() {
        let json = r#"{
            "agent": "frontend",
            "description": "React 앱 담당. 컴포넌트/라우팅/상태관리 질문은 나에게",
            "max_inline_bytes": 262144,
            "content_types": ["text/*", "application/json"],
            "encodings": ["json"],
            "modes": ["poll"],
            "meta": {}
        }"#;
        let caps: Capabilities = serde_json::from_str(json).unwrap();
        assert_eq!(caps.modes, vec![ReceiveMode::Poll]);
        assert_eq!(caps.max_inline_bytes, 262_144);
    }
}
