//! brv 설정 — 파일(비밀 제외) + OS 키체인(토큰).
//!
//! 원칙(PLAN·PROTOCOL 5.1): 클라이언트에 비밀 없음 — 토큰은 OS 키체인에 보관하고
//! 설정 파일에는 넣지 않는다. 키체인이 없는 환경(일부 headless 리눅스)은
//! `BREVDUVA_TOKEN` 환경 변수로 대체할 수 있다.

use std::path::PathBuf;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// 설정 파일 본문 (`~/.config/brevduva/config.toml` 또는 %APPDATA%\brevduva\config.toml).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrvConfig {
    /// 서버 베이스 URL (http/https) — WS 주소는 여기서 유도한다.
    pub server: String,
    pub channel: String,
    pub agent: String,
    /// 능력 선언의 소개문 (PROTOCOL.md 4장) — 동료가 라우팅 판단에 쓴다.
    #[serde(default)]
    pub description: String,
}

pub fn config_path() -> anyhow::Result<PathBuf> {
    // 한 머신에서 여러 에이전트 프로필을 돌릴 때(테스트·다중 프로젝트)를 위한 오버라이드
    if let Ok(path) = std::env::var("BREVDUVA_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    let dir = dirs::config_dir().context("cannot resolve OS config directory")?;
    Ok(dir.join("brevduva").join("config.toml"))
}

pub fn load() -> anyhow::Result<BrvConfig> {
    let path = config_path()?;
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("config not found at {path:?} — run `brv init` first"))?;
    toml::from_str(&text).context("config parse failed")
}

pub fn store(cfg: &BrvConfig) -> anyhow::Result<PathBuf> {
    let path = config_path()?;
    std::fs::create_dir_all(path.parent().expect("config path has parent"))?;
    std::fs::write(&path, toml::to_string_pretty(cfg)?)?;
    Ok(path)
}

fn keyring_entry(cfg: &BrvConfig) -> anyhow::Result<keyring::Entry> {
    // 계정 키에 서버·채널·에이전트를 모두 넣어 다중 프로필 충돌을 막는다
    keyring::Entry::new(
        "brevduva",
        &format!("{}:{}@{}", cfg.channel, cfg.agent, cfg.server),
    )
    .context("keyring unavailable")
}

/// 토큰 저장 — 키체인 우선. 실패 시 오류 (파일에 평문 저장은 하지 않는다).
pub fn store_token(cfg: &BrvConfig, token: &str) -> anyhow::Result<()> {
    keyring_entry(cfg)?
        .set_password(token)
        .context("keyring write failed — set BREVDUVA_TOKEN env var as a fallback")
}

/// 토큰 로드 — `BREVDUVA_TOKEN` 환경 변수가 키체인보다 우선 (CI·headless용).
pub fn load_token(cfg: &BrvConfig) -> anyhow::Result<String> {
    if let Ok(token) = std::env::var("BREVDUVA_TOKEN") {
        return Ok(token);
    }
    keyring_entry(cfg)?
        .get_password()
        .context("token not found — run `brv init` or set BREVDUVA_TOKEN")
}
