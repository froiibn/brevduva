//! 일회용 연결 코드 교환 (PROTOCOL.md 10.1) — `brv init --enroll`의 코어.
//!
//! 여기는 교환과 설정 구성만 담당한다 — 저장(키체인/파일)·안내 출력·MCP 등록은
//! main이 맡는다. 이 분리로 서버 리포의 통합 테스트가 실제 교환 경로를 그대로 검증한다.

use anyhow::Context as _;

use crate::config::BrvConfig;

pub struct Enrolled {
    /// 저장할 설정 — channel은 부여받은 채널 중 선택된 하나, [wake]는 기존 설정에서 보존.
    pub cfg: BrvConfig,
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
            "enroll 거부: {}",
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
            "이 코드에 참가 채널이 없습니다 — 대시보드에서 채널을 지정해 다시 발급하세요",
        )?,
    };
    // 기존 설정의 [wake]는 보존 — 재설치·재연결이 깨우기 설정을 지우면 안 된다 (init과 동일 원칙)
    let wake = crate::config::load()
        .ok()
        .and_then(|existing| existing.wake);
    Ok(Enrolled {
        cfg: BrvConfig {
            server: base,
            channel: chosen,
            agent,
            description,
            wake,
        },
        token,
        channels,
        org,
    })
}
