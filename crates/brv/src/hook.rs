// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! Claude Code Stop 훅 (페이즈 17) — "턴이 끝날 때 우편함 확인".
//!
//! 대화형 세션이 떠 있는 동안 데몬은 standby(2.2)라 깨우기가 없다 — 그 사각지대를
//! 세션 자신의 턴 종료 훅으로 메운다: 대기 메시지가 있으면 Stop을 막고(decision: block)
//! 수신 처리를 지시한다. 확인은 **비파괴 peek**(`GET /v1/frames?peek=true`, PROTOCOL 7장) —
//! 훅이 메시지를 집어가면 곧 이어질 wait_for_message가 ack_wait 재전달을 기다리는 경합이
//! 생기므로, 보되 잡지 않는다.
//!
//! **다중 바인딩 (페이즈 27)**: 훅은 머신 전역(사용자 설정)이라 어느 바인딩의 세션에서
//! 도는지 모른다 — 전 바인딩을 peek해 합산하고, 사유에 바인딩별 대기 수를 병기한다.
//! 세션은 자기 MCP 바인딩 몫만 처리할 수 있다 (다른 몫은 데몬 wake가 감당).

use anyhow::Context as _;

/// peek 대상 하나 — 바인딩과 그 토큰.
pub struct HookTarget {
    pub agent: String,
    pub channel: String,
    pub token: String,
}

/// 대기 중인 동료 메시지 수 (peek — 큐 불변). `_system` 이벤트는 세지 않는다.
pub async fn pending_peer_messages(
    server: &str,
    channel: &str,
    token: &str,
) -> anyhow::Result<usize> {
    let resp = reqwest::Client::new()
        .get(format!(
            "{}/v1/frames?channel={channel}&wait=0&peek=true",
            server.trim_end_matches('/'),
        ))
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .context("server unreachable")?;
    anyhow::ensure!(
        resp.status().is_success(),
        "peek rejected: {}",
        resp.status()
    );
    let frames: Vec<serde_json::Value> = resp.json().await.context("peek body")?;
    Ok(frames
        .iter()
        .filter(|f| f["op"] == "DELIVER" && f["body"]["from"] != "_system")
        .count())
}

/// Stop 훅 진입점 — stdin의 훅 입력(JSON)을 읽고, 필요하면 block 결정을 stdout으로.
/// 실패는 침묵(정상 종료) — 훅이 서버 장애를 세션 장애로 번지게 하면 안 된다.
/// 개별 바인딩의 peek 실패도 0으로 취급해 나머지 바인딩 판정을 막지 않는다.
pub async fn stop(server: &str, targets: &[HookTarget], stdin_json: &str) -> Option<String> {
    // 재진입 가드: 우리가 이미 한 번 막아서 이어진 턴이면 다시 막지 않는다 (무한 루프 방지)
    if serde_json::from_str::<serde_json::Value>(stdin_json)
        .ok()
        .is_some_and(|v| v["stop_hook_active"] == true)
    {
        return None;
    }
    let mut per_binding = Vec::new();
    let mut total = 0usize;
    for t in targets {
        let n = pending_peer_messages(server, &t.channel, &t.token)
            .await
            .unwrap_or(0);
        if n > 0 {
            per_binding.push(format!("{}@{}: {n}", t.agent, t.channel));
            total += n;
        }
    }
    if total == 0 {
        return None;
    }
    Some(
        serde_json::json!({
            "decision": "block",
            "reason": format!(
                "Brevduva: {total} message(s) from peer agents are waiting ({}). \
                 Receive the ones for this session's agent with the brevduva MCP tool \
                 wait_for_message and handle them (reply/acknowledge/report per the \
                 collaboration contract) before finishing. Messages for other bindings \
                 are handled by their own sessions or the daemon.",
                per_binding.join(", ")
            ),
        })
        .to_string(),
    )
}

/// `brv hook install` — Claude Code 사용자 설정(~/.claude/settings.json)에 Stop 훅 등록.
/// 멱등: 이미 등록돼 있으면 그대로 둔다. 다른 훅·설정은 건드리지 않는다 (병합만).
pub fn install() -> anyhow::Result<String> {
    let path = dirs::home_dir()
        .context("cannot resolve home directory")?
        .join(".claude")
        .join("settings.json");
    let mut root: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("{} is not valid JSON — fix it first", path.display()))?,
        Err(_) => serde_json::json!({}),
    };
    let stops = root
        .as_object_mut()
        .context("settings root must be an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("hooks must be an object")?
        .entry("Stop")
        .or_insert_with(|| serde_json::json!([]));
    let already = stops.as_array().is_some_and(|entries| {
        entries.iter().any(|e| {
            e["hooks"]
                .as_array()
                .is_some_and(|hs| hs.iter().any(|h| h["command"] == "brv hook stop"))
        })
    });
    if already {
        return Ok(format!("already registered — {}", path.display()));
    }
    stops
        .as_array_mut()
        .context("hooks.Stop must be an array")?
        .push(serde_json::json!({
            "hooks": [{ "type": "command", "command": "brv hook stop" }]
        }));
    std::fs::create_dir_all(path.parent().expect("settings path has parent"))?;
    std::fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    Ok(format!(
        "Stop hook registered — {} (Claude Code sessions check pending messages at each turn end)",
        path.display()
    ))
}
