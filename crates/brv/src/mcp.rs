// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! MCP 도구 인자 공용 도우미 — 리시버의 로컬 평면(`local_plane::plane`)과 관리 도구(`manage`)가 쓴다.
//!
//! 세션 프로세스 안에서 서버에 붙던 MCP 서버(`McpServer`)와 러너별 전달 어댑터(Claude Channels·
//! Codex 공유 앱 서버·세션 소유 Monitor/queue)는 2026-09-11 삭제됐다(RECEIVER_REBUILD_PLAN 7e) —
//! 세션은 리시버에 붙고(`brv mcp` = 브리지, P3), 도구와 전달은 리시버의 평면이 처리한다(P2).

use serde_json::{Value, json};

pub(crate) fn missing(field: &str) -> (Value, bool) {
    (
        json!({ "status": "error", "message": format!("missing required argument {field:?}") }),
        true,
    )
}

/// `to` 표기 편의: 접두 없는 이름은 지명 전달로 해석 (`agent:` 자동 부여).
pub(crate) fn normalize_to(to: &str) -> String {
    if to == "broadcast" || to.starts_with("agent:") || to.starts_with("topic:") {
        to.to_owned()
    } else {
        format!("agent:{to}")
    }
}

/// 불리언 도구 인자 — 호스트마다 직렬화가 다르다 (2026-09-05 실측: 한 MCP 호스트가 `newest_first`를
/// 문자열 `"true"`로 보내 서버가 false로 읽었다). JSON 불리언 외에 `"true"/"false"`·`"1"/"0"`·`1/0`도 받는다.
/// 모르는 값은 false — 뜻을 지어내지 않는다.
pub fn bool_arg(args: &Value, key: &str) -> bool {
    match &args[key] {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64() == Some(1),
        Value::String(s) => matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-05: 호스트가 불리언을 문자열로 보내도 뜻을 잃지 않는다 — 모르는 값은 false.
    #[test]
    fn bool_args_accept_host_serialization_variants() {
        let v = serde_json::json!({ "a": true, "b": "true", "c": "TRUE", "d": 1, "e": "1", "f": "false", "g": 0, "h": "maybe", "i": null });
        assert!(
            bool_arg(&v, "a")
                && bool_arg(&v, "b")
                && bool_arg(&v, "c")
                && bool_arg(&v, "d")
                && bool_arg(&v, "e")
        );
        assert!(
            !bool_arg(&v, "f")
                && !bool_arg(&v, "g")
                && !bool_arg(&v, "h")
                && !bool_arg(&v, "i")
                && !bool_arg(&v, "missing")
        );
    }

    #[test]
    fn to_normalization() {
        assert_eq!(normalize_to("frontend"), "agent:frontend");
        assert_eq!(normalize_to("agent:frontend"), "agent:frontend");
        assert_eq!(normalize_to("broadcast"), "broadcast");
        assert_eq!(normalize_to("topic:a.b"), "topic:a.b");
    }
}
