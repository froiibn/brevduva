// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 일회용 연결 코드 교환 (PROTOCOL.md 10.1) — `brv init --enroll`의 코어.
//!
//! 여기는 교환만 담당한다 — 설정 병합(바인딩 추가, 페이즈 27)·저장(키체인/파일)·
//! 안내 출력·MCP 등록은 main이 맡는다. 이 분리로 서버 리포의 통합 테스트가 실제
//! 교환 경로를 그대로 검증한다.
//!
//! 다중 형식(2026-09-02, 에이전트 연결 화면 재설계): 코드 하나가 에이전트 N명의
//! (에이전트, 채널) 쌍 전부를 지정하면 응답에 `agents` 배열이 오고, 여기서 바인딩을
//! 전부 만든다. 구형 단일 응답은 선택된 채널 하나로만 바인딩한다 (나머지는 `brv binding add`).

use anyhow::Context as _;

use crate::config::Binding;

/// 코드로 연결된 에이전트 하나 — 토큰은 에이전트당 하나, 바인딩은 채널당 하나.
#[derive(Debug)]
pub struct EnrolledAgent {
    pub token: String,
    /// 설정에 추가할 바인딩들. wake_dir·wake_policy는 기본값 — 기존 바인딩 교체 시
    /// main의 upsert가 보존한다.
    pub bindings: Vec<Binding>,
    /// 이 에이전트에 grant된 전체 채널 (안내 출력용 — 구형 코드에서는 bindings보다 많을 수 있다).
    pub channels: Vec<String>,
}

#[derive(Debug)]
pub struct Enrolled {
    /// 교환에 쓴 서버 베이스 URL (정규화됨) — 설정 병합 시 서버 일치 검증에 쓴다.
    pub server: String,
    pub org: String,
    pub agents: Vec<EnrolledAgent>,
}

/// 코드 → 토큰 교환. 채널 선택은 서버가 **소모 전에** 검증한다(10.1) —
/// 잘못된 --channel이 일회용 코드를 태우지 않는다.
pub async fn exchange(server: &str, code: &str, channel: Option<&str>) -> anyhow::Result<Enrolled> {
    // 실사고(2026-09-02): 대시보드의 에이전트 토큰을 코드로 오인 — 서버 왕복 전에 즉시 안내
    anyhow::ensure!(
        !code.starts_with("brv_"),
        "this looks like an agent token, not an enroll code — issue an enroll code in the dashboard (Connect an agent)"
    );
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
    let org = body["org"].as_str().unwrap_or_default().to_owned();
    let agents = match body["agents"].as_array().filter(|list| !list.is_empty()) {
        // 다중 형식 — 코드가 지정한 (에이전트, 채널) 쌍 전부
        Some(list) => {
            let mut out = Vec::with_capacity(list.len());
            for entry in list {
                let agent = entry["agent"]
                    .as_str()
                    .context("no agent in response")?
                    .to_owned();
                let token = entry["token"]
                    .as_str()
                    .context("no token in response")?
                    .to_owned();
                let description = entry["description"].as_str().unwrap_or_default();
                let channels = str_list(&entry["channels"]);
                anyhow::ensure!(
                    !channels.is_empty(),
                    "agent {agent}: this code carries no channels — reissue it in the dashboard with channels attached"
                );
                let bindings = channels
                    .iter()
                    .map(|ch| binding_for(&org, &agent, ch, description))
                    .collect();
                out.push(EnrolledAgent {
                    token,
                    bindings,
                    channels,
                });
            }
            out
        }
        // 구형 단일 응답 — 선택 채널 하나 (서버가 소모 전에 검증했으므로 여기 도달했다면 유효)
        None => {
            let agent = body["agent"]
                .as_str()
                .context("no agent in response")?
                .to_owned();
            let token = body["token"]
                .as_str()
                .context("no token in response")?
                .to_owned();
            let description = body["description"].as_str().unwrap_or_default();
            let channels = str_list(&body["channels"]);
            let chosen = match channel {
                Some(c) => c.to_owned(),
                None => channels.first().cloned().context(
                    "this code carries no channels — reissue it in the dashboard with channels attached",
                )?,
            };
            vec![EnrolledAgent {
                token,
                bindings: vec![binding_for(&org, &agent, &chosen, description)],
                channels,
            }]
        }
    };
    Ok(Enrolled {
        server: base,
        org,
        agents,
    })
}

fn str_list(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// 교환 결과 → 바인딩. org를 새긴다 (2026-09-01) — 조직 간 동명 에이전트의 토큰 키·선택자 구분.
fn binding_for(org: &str, agent: &str, channel: &str, description: &str) -> Binding {
    Binding {
        org: (!org.is_empty()).then(|| org.to_owned()),
        agent: agent.to_owned(),
        channel: channel.to_owned(),
        description: description.to_owned(),
        wake_dir: None,
        wake_policy: "always".to_owned(),
        wake_command: None,
        wake_args: None,
    }
}
