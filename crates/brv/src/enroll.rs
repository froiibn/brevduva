// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 일회용 연결 코드 교환 (PROTOCOL.md 10.1) — `brv init --enroll`의 코어.
//!
//! 여기는 교환만 담당한다 — 설정 병합(바인딩 추가, 페이즈 27)·저장(키체인/파일)·
//! 안내 출력·MCP 등록은 main이 맡는다. 이 분리로 서버 리포의 통합 테스트가 실제
//! 교환 경로를 그대로 검증한다.

use anyhow::Context as _;

use crate::config::Binding;

pub struct Enrolled {
    /// 교환에 쓴 서버 베이스 URL (정규화됨) — 설정 병합 시 서버 일치 검증에 쓴다.
    pub server: String,
    /// 설정에 추가할 바인딩 — channel은 부여받은 채널 중 선택된 하나.
    /// wake_dir·wake_policy는 기본값 — 기존 바인딩 교체 시 main의 upsert가 보존한다.
    pub binding: Binding,
    pub token: String,
    /// 이 코드로 grant된 전체 채널 목록 (안내 출력용).
    pub channels: Vec<String>,
    pub org: String,
}

/// 코드 → 토큰 교환. 채널 선택은 서버가 **소모 전에** 검증한다(10.1) —
/// 잘못된 --channel이 일회용 코드를 태우지 않는다.
pub async fn exchange(server: &str, code: &str, channel: Option<&str>) -> anyhow::Result<Enrolled> {
    let base = server.trim_end_matches('/').to_owned();
    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/enroll"))
        .json(&serde_json::json!({ "code": code, "channel": channel }))
        .send()
        .await
        .context("server unreachable")?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "enroll rejected: {}",
            body["message"]
                .as_str()
                .unwrap_or("invalid or expired code")
        );
    }
    let agent = body["agent"]
        .as_str()
        .context("no agent in response")?
        .to_owned();
    let token = body["token"]
        .as_str()
        .context("no token in response")?
        .to_owned();
    let org = body["org"].as_str().unwrap_or_default().to_owned();
    let description = body["description"].as_str().unwrap_or_default().to_owned();
    let channels: Vec<String> = body["channels"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let chosen = match channel {
        // 서버가 소모 전에 검증했으므로 여기 도달했다면 유효
        Some(c) => c.to_owned(),
        None => channels.first().cloned().context(
            "this code carries no channels — reissue it in the dashboard with channels attached",
        )?,
    };
    Ok(Enrolled {
        server: base,
        binding: Binding {
            // org를 바인딩에 새긴다 (2026-09-01) — 조직 간 동명 에이전트의 토큰 키·선택자 구분
            org: (!org.is_empty()).then(|| org.clone()),
            agent,
            channel: chosen,
            description,
            wake_dir: None,
            wake_policy: "always".to_owned(),
            wake_command: None,
            wake_args: None,
        },
        token,
        channels,
        org,
    })
}
