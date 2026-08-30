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
    /// `always` | `never` (5.3: 항상/무시. 업무시간 정책은 2026-08-29 기각 —
    /// 에이전트에 사람 리듬 투영은 모순, 비용 우려는 수요 확인 시 깨우기 빈도 상한으로)
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
    // 기본 = 가장 좁은 respond 프리셋 (페이즈 21에서 프리셋으로 일원화)
    wake_preset_args("respond").expect("respond preset exists")
}
fn default_wake_timeout() -> u64 {
    600
}

/// 깨우기 권한 프리셋 (페이즈 21) — 무인(headless) 세션에 **사전 허용**할 도구 수준.
/// 무인 실행은 권한을 물어볼 사람이 없어 이 목록이 전부다 — 기본은 가장 좁은 respond.
/// 이 값은 로컬 신뢰 정책이라 의도적으로 설정 파일에만 산다 (서버·원격 메시지가 변경 불가).
/// 프리셋은 Claude Code CLI 기준 — 다른 러너는 args를 직접 편집한다 (README 무인 모드 절).
pub fn wake_preset_tools(level: &str) -> Option<&'static str> {
    Some(match level {
        "respond" => "mcp__brevduva__*",
        "edit" => "mcp__brevduva__*,Read,Glob,Grep,Edit,Write",
        "full" => "mcp__brevduva__*,Read,Glob,Grep,Edit,Write,Bash",
        _ => return None,
    })
}

/// 프리셋 → 실행 인자 열. `brv wake set --allow`와 기본값이 공유하는 단일 원천.
pub fn wake_preset_args(level: &str) -> Option<Vec<String>> {
    let tools = wake_preset_tools(level)?;
    Some(vec![
        "-p".to_owned(),
        "{prompt}".to_owned(),
        "--allowedTools".to_owned(),
        tools.to_owned(),
    ])
}

/// args의 `--allowedTools` 값 — `wake show` 표시와 깨우기 프롬프트의 정직성 라인이 쓴다.
pub fn wake_allowed_tools(args: &[String]) -> Option<&str> {
    args.iter()
        .position(|a| a == "--allowedTools")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// args가 어느 프리셋 산출물인지 역판별 (`wake show`용) — 손 편집분은 None(custom).
pub fn wake_preset_of(args: &[String]) -> Option<&'static str> {
    ["respond", "edit", "full"]
        .into_iter()
        .find(|level| wake_preset_args(level).as_deref() == Some(args))
}

/// 프로세스 내 설정 경로 고정 — 윈도우 서비스 모드(페이즈 7)에서 SCM launch args로 받은
/// 경로를 전달하는 통로. 에디션 2024에서 `env::set_var`가 unsafe라 env 주입 대신 이 방식.
static PATH_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn set_path_override(path: PathBuf) {
    let _ = PATH_OVERRIDE.set(path);
}

pub fn config_path() -> anyhow::Result<PathBuf> {
    // 우선순위: 프로세스 내 고정(서비스 모드) → env(다중 프로필) → OS 기본 경로
    if let Some(path) = PATH_OVERRIDE.get() {
        return Ok(path.clone());
    }
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

/// 토큰이 실제로 저장된 위치 — 호출자가 사용자에게 정확히 알려주기 위한 반환값.
#[derive(Debug)]
pub enum TokenStore {
    Keyring,
    File(PathBuf),
}

/// 토큰 저장 — 키체인 우선, 불가하면 토큰 파일(600) 폴백.
/// 키체인 없는 헤드리스 리눅스에서 init이 실패하던 결함의 근본 수정 (2026-08-28, 페이즈 10) —
/// load_token의 파일 폴백과 대칭이 됐다 (lucadm 배치 때는 수동으로 우회했던 경로).
pub fn store_token(cfg: &BrvConfig, token: &str) -> anyhow::Result<TokenStore> {
    if let Ok(entry) = keyring_entry(cfg)
        && entry.set_password(token).is_ok()
    {
        return Ok(TokenStore::Keyring);
    }
    let path = config_path()?
        .parent()
        .expect("config has parent")
        .join("token");
    std::fs::create_dir_all(path.parent().expect("token path has parent"))?;
    std::fs::write(&path, token)
        .with_context(|| format!("keyring unavailable and token file write failed at {path:?}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(TokenStore::File(path))
}

/// 토큰 로드 우선순위: `BREVDUVA_TOKEN` env → OS 키체인 → 토큰 파일.
/// 토큰 파일(설정 디렉터리의 `token`, 600 권한)은 키체인이 없는 headless 리눅스 서버용 —
/// 데몬이 깨운 세션의 MCP도 같은 경로로 토큰을 찾는다 (2026-08-27, lucadm 배치에서 필요 확인).
pub fn load_token(cfg: &BrvConfig) -> anyhow::Result<String> {
    if let Ok(token) = std::env::var("BREVDUVA_TOKEN") {
        return Ok(token);
    }
    if let Ok(entry) = keyring_entry(cfg)
        && let Ok(token) = entry.get_password()
    {
        return Ok(token);
    }
    let token_file = config_path()?
        .parent()
        .expect("config has parent")
        .join("token");
    std::fs::read_to_string(&token_file)
        .map(|t| t.trim().to_owned())
        .with_context(|| {
            format!("token not found — run `brv init`, set BREVDUVA_TOKEN, or place it at {token_file:?}")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_presets_widen_monotonically() {
        // 프리셋은 좁은 것부터 넓어지는 포함 관계 — respond ⊂ edit ⊂ full
        let respond = wake_preset_tools("respond").unwrap();
        let edit = wake_preset_tools("edit").unwrap();
        let full = wake_preset_tools("full").unwrap();
        assert!(edit.starts_with(respond));
        assert!(full.starts_with(edit));
        assert!(full.contains("Bash") && !edit.contains("Bash"));
        assert!(edit.contains("Edit") && !respond.contains("Edit"));
        assert!(
            wake_preset_tools("root").is_none(),
            "unknown level rejected"
        );
    }

    #[test]
    fn default_args_are_respond_and_roundtrip() {
        // 기본값 = respond 프리셋 (단일 원천), 역판별·도구 추출이 왕복한다
        let args = default_wake_args();
        assert_eq!(wake_preset_of(&args), Some("respond"));
        assert_eq!(wake_allowed_tools(&args), Some("mcp__brevduva__*"));
        assert!(args.iter().any(|a| a == "{prompt}"));
        // 손 편집분(순서·값이 다름)은 custom 취급
        let custom = vec!["-p".to_owned(), "{prompt}".to_owned()];
        assert_eq!(wake_preset_of(&custom), None);
        assert_eq!(wake_allowed_tools(&custom), None);
    }

    #[test]
    fn wake_config_survives_toml_roundtrip() {
        // "한번 설정하면 유지"의 최소 보증 — 직렬화 왕복에서 필드가 안 사라진다
        // (재init 보존은 main의 init이 load→wake 이식으로 담당, 회귀는 phase10 e2e)
        let cfg = BrvConfig {
            server: "https://api.brevduva.dev".into(),
            channel: "myapp".into(),
            agent: "backend".into(),
            description: String::new(),
            wake: Some(WakeConfig {
                policy: "always".into(),
                command: "/home/user/.local/bin/claude".into(),
                args: wake_preset_args("full").unwrap(),
                dir: "/home/user/app".into(),
                timeout_s: 900,
            }),
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: BrvConfig = toml::from_str(&text).unwrap();
        let wake = back.wake.unwrap();
        assert_eq!(wake_preset_of(&wake.args), Some("full"));
        assert_eq!(wake.timeout_s, 900);
        assert_eq!(wake.command, "/home/user/.local/bin/claude");
    }
}
