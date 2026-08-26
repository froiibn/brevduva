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
    /// `brv daemon`의 세션 깨우기 설정 (5.3 CLI 어댑터 규약). 없으면 daemon 기동 거부.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<WakeConfig>,
}

/// 깨우기 설정 — 메시지 도착 시 실행할 명령 (예: `claude -p "{prompt}"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeConfig {
    /// `always` | `never` (5.3: 항상/무시. 업무시간 정책은 잔여)
    #[serde(default = "default_policy")]
    pub policy: String,
    /// 실행 파일 (전체 경로 권장 — PATH에 없을 수 있음)
    pub command: String,
    /// 인자 목록 — `{prompt}` 자리에 메시지 프롬프트가 치환된다
    #[serde(default = "default_wake_args")]
    pub args: Vec<String>,
    /// 실행 작업 디렉터리 (해당 프로젝트 루트 — .mcp.json이 있는 곳)
    pub dir: String,
    /// 깨운 세션의 최대 실행 시간(초) — 초과 시 강제 종료
    #[serde(default = "default_wake_timeout")]
    pub timeout_s: u64,
}

fn default_policy() -> String {
    "always".to_owned()
}
fn default_wake_args() -> Vec<String> {
    vec![
        "-p".to_owned(),
        "{prompt}".to_owned(),
        "--allowedTools".to_owned(),
        "mcp__brevduva__*".to_owned(),
    ]
}
fn default_wake_timeout() -> u64 {
    600
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
    // 계정 키 = 에이전트@서버 — 채널은 넣지 않는다 (2026-08-27 수정).
    // 토큰은 org 스코프(PROTOCOL.md 2.1·5.1)라 한 에이전트가 여러 채널에 참가한다 —
    // 채널을 키에 넣으면 채널 추가마다 토큰을 못 찾는 스펙 위반이 된다 (실전 채널 분리에서 발견)
    keyring::Entry::new("brevduva", &format!("{}@{}", cfg.agent, cfg.server))
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
