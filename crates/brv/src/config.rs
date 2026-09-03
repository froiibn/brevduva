// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! brv 설정 — 파일(비밀 제외) + 토큰 보관처(OS 키체인 또는 토큰 파일).
//!
//! 원칙(PLAN·PROTOCOL 5.1): 클라이언트에 비밀 없음 — 토큰은 설정 파일 본문에 넣지 않는다.
//! 보관처는 플랫폼마다 하나만 **주 저장소**다 (`keychain_is_reliable`): 서명된 맥 빌드는
//! 키체인, 그 외 — **윈도우**(2026-09-03, 데몬이 LocalSystem 서비스라 사용자 자격 증명
//! 저장소를 못 본다)·**리눅스**(2026-09-04, systemd 사용자 유닛+linger는 부팅 시 세션 키링
//! 없이 기동한다)·서명 없는 맥 빌드(프롬프트 폭풍) — 는 설정 디렉터리의 토큰 파일이다.
//! 다른 쪽에 남은 토큰은 읽을 때 주 저장소로 **이전**한다(쓰고 되읽어 확인한 뒤에야 원본
//! 삭제 — `load_token`). `BREVDUVA_TOKEN` 환경 변수도 대체 경로다(단일 바인딩 전용).
//!
//! 파일 보관은 곧 **평문 보관**이라 접근 통제가 저장의 일부다 — `secure_config_dir()`가
//! 설정 디렉터리를 소유자 전용으로 좁힌다(유닉스 0700, 윈도우 DACL). 토큰뿐 아니라
//! 저널(주고받은 메시지 본문)이 같은 폴더에 살기 때문에 파일이 아니라 디렉터리 단위다.
//!
//! **다중 바인딩 (페이즈 27)**: 한 머신의 brv 프로세스 하나가 여러 (에이전트, 채널)
//! 바인딩을 수신한다. 설정은 전역(server, `[wake]` 공통)과 `[[binding]]` 배열로 나뉜다 —
//! wake의 실행기·권한·타임아웃은 머신의 로컬 신뢰 정책이라 전역, 작업 디렉터리와
//! 깨우기 여부는 프로젝트 속성이라 바인딩별. 페이즈 27 이전의 단수형(톱레벨
//! channel/agent + [wake]의 dir/policy)은 읽기 시 바인딩 1개로 해석한다 (하위 호환 —
//! 기존 머신은 재설정 불요, 다음 저장 때 신형으로 이행).

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// (에이전트, 채널) 바인딩 하나 — 이 머신이 수행하는 정체성 (페이즈 27).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Binding {
    /// 소속 조직 — 여러 조직의 동명 에이전트·채널 구분용 (2026-09-01 보강).
    /// 채널·에이전트 이름은 org 스코프라, org 없이는 조직 간 충돌 시 토큰·선택자가 겹친다.
    /// enroll이 채우며, 구형 설정(단일 조직 시절)에는 없다 → None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    pub agent: String,
    pub channel: String,
    /// 능력 선언의 소개문 (PROTOCOL.md 4장) — 동료가 라우팅 판단에 쓴다.
    #[serde(default)]
    pub description: String,
    /// 깨어난 세션의 작업 디렉터리 (해당 프로젝트 루트 — .mcp.json이 있는 곳).
    /// 없으면 이 바인딩은 wake 불가 — 데몬 기동 시 검증한다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_dir: Option<String>,
    /// 이 바인딩 전용 깨우기 실행 파일 — 없으면 전역 `[wake].command` 상속.
    /// 러너 혼용용 (2026-09-01): 한 머신에서 claude 바인딩과 codex 바인딩을 함께 굴린다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_command: Option<String>,
    /// 이 바인딩 전용 깨우기 인자 — 없으면 전역 `[wake].args` 상속 (`{prompt}` 치환 동일).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_args: Option<Vec<String>>,
}

impl Binding {
    /// 표시·선택자 표기 — `agent@channel`.
    pub fn label(&self) -> String {
        format!("{}@{}", self.agent, self.channel)
    }

    /// org까지 붙인 완전 표기 — `org/agent@channel` (org 미상이면 label과 동일).
    pub fn full_label(&self) -> String {
        match &self.org {
            Some(o) => format!("{o}/{}", self.label()),
            None => self.label(),
        }
    }

    /// 토큰 저장·조회의 정체성 키 — org를 알면 `org/agent` (조직 간 동명 에이전트 구분),
    /// 모르면 `agent` (구형 호환 — 기존 키체인 항목이 이 형태).
    pub fn token_id(&self) -> String {
        match &self.org {
            Some(o) => format!("{o}/{}", self.agent),
            None => self.agent.clone(),
        }
    }
}

/// 설정 파일 본문 (`~/.config/brevduva/config.toml` 또는 %APPDATA%\brevduva\config.toml).
#[derive(Debug, Clone, Serialize)]
pub struct BrvConfig {
    /// 서버 베이스 URL (http/https) — WS 주소는 여기서 유도한다. 머신당 서버 하나
    /// (프로필 분리는 `BREVDUVA_CONFIG`로 — 페이즈 27 결정).
    pub server: String,
    /// `brv daemon`의 세션 깨우기 실행기 (5.3 CLI 어댑터 규약). 없으면 daemon 기동 거부.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake: Option<WakeConfig>,
    /// 이 머신의 바인딩들 — 데몬은 전부 동시 수신, 단일 대상 명령은 `--binding`으로 선택.
    #[serde(rename = "binding")]
    pub bindings: Vec<Binding>,
}

/// 전역 깨우기 설정 — 무엇으로 깨울지 (예: `claude -p "{prompt}"`).
/// 실행기·권한(args)·타임아웃은 머신의 로컬 신뢰 정책이라 바인딩과 무관하게 전역이다.
/// 작업 디렉터리·정책은 바인딩 소관 (페이즈 27 분리).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeConfig {
    /// 실행 파일 (전체 경로 권장 — PATH에 없을 수 있음)
    pub command: String,
    /// 인자 목록 — `{prompt}` 자리에 메시지 프롬프트가 치환된다
    #[serde(default = "default_wake_args")]
    pub args: Vec<String>,
    /// 깨운 세션의 최대 실행 시간(초) — 초과 시 강제 종료
    #[serde(default = "default_wake_timeout")]
    pub timeout_s: u64,
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

impl BrvConfig {
    /// 바인딩 선택 — 명령의 `--binding` 값(있으면)으로, 없으면 유일한 바인딩으로.
    /// 복수 바인딩에서 미지정은 에러 — 조용히 엉뚱한 채널로 나가는 것보다 낫다 (페이즈 27).
    pub fn select(&self, selector: Option<&str>) -> anyhow::Result<&Binding> {
        match selector {
            Some(sel) => self.find(sel),
            None => match self.bindings.as_slice() {
                [] => {
                    anyhow::bail!("no bindings configured — run `brv init --enroll <code>` first")
                }
                [one] => Ok(one),
                many => anyhow::bail!(
                    "multiple bindings configured — pick one with --binding: {}",
                    many.iter()
                        .map(Binding::label)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
        }
    }

    /// 선택자 해석 — 좁은 것부터: `org/agent@channel` > `agent@channel` > `org/agent` >
    /// `agent`. 생략된 부분은 조건에서 빠지며, 결과가 유일하지 않으면 더 구체적인 형태를
    /// 요구하는 에러를 낸다 (조직 간 동명 충돌 대비, 2026-09-01).
    pub fn find(&self, selector: &str) -> anyhow::Result<&Binding> {
        let (org, rest) = match selector.split_once('/') {
            Some((o, r)) => (Some(o), r),
            None => (None, selector),
        };
        let (agent, channel) = match rest.split_once('@') {
            Some((a, c)) => (a, Some(c)),
            None => (rest, None),
        };
        let mut hits = self.bindings.iter().filter(|b| {
            b.agent == agent
                && channel.is_none_or(|c| b.channel == c)
                && org.is_none_or(|o| b.org.as_deref() == Some(o))
        });
        let first = hits
            .next()
            .with_context(|| format!("no binding matches {selector:?} — see `brv binding list`"))?;
        anyhow::ensure!(
            hits.next().is_none(),
            "selector {selector:?} is ambiguous — be more specific (org/agent@channel): {}",
            self.bindings
                .iter()
                .map(Binding::full_label)
                .collect::<Vec<_>>()
                .join(", ")
        );
        Ok(first)
    }

    /// 바인딩 추가/교체 — 같은 (org, agent, channel)이 있으면 소개문만 갱신하고 운영 설정
    /// (wake_dir·wake_command·wake_args)은 보존한다 (재init·재enroll이
    /// 깨우기를 지우면 안 됨). org가 한쪽만 있으면 같은 것으로 보고 채운다 —
    /// 구형(단일 조직) 바인딩이 재enroll로 org를 획득하는 경로. 반환: true = 기존 교체.
    pub fn upsert_binding(&mut self, incoming: Binding) -> bool {
        if let Some(existing) = self.bindings.iter_mut().find(|b| {
            b.agent == incoming.agent
                && b.channel == incoming.channel
                && (b.org.is_none() || incoming.org.is_none() || b.org == incoming.org)
        }) {
            if incoming.org.is_some() {
                existing.org = incoming.org;
            }
            if !incoming.description.is_empty() {
                existing.description = incoming.description;
            }
            true
        } else {
            self.bindings.push(incoming);
            false
        }
    }
}

/// 프로세스 내 설정 경로 고정 — 윈도우 서비스 모드(페이즈 7)에서 SCM launch args로 받은
/// 경로를 전달하는 통로. 에디션 2024에서 `env::set_var`가 unsafe라 env 주입 대신 이 방식.
static PATH_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn set_path_override(path: PathBuf) {
    let _ = PATH_OVERRIDE.set(path);
}

pub fn config_path() -> anyhow::Result<PathBuf> {
    // 우선순위: 프로세스 내 고정(서비스 모드) → env(다중 프로필) → 등록된 OS 서비스의 프로필
    // (2026-09-04 — 서비스가 있는 머신의 "이 머신 프로필"은 그것이다) → OS 기본 경로
    if let Some(path) = PATH_OVERRIDE.get() {
        return Ok(path.clone());
    }
    if let Ok(path) = std::env::var("BREVDUVA_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = registered_service_config() {
        return Ok(path.clone());
    }
    let dir = dirs::config_dir().context("cannot resolve OS config directory")?;
    Ok(dir.join("brevduva").join("config.toml"))
}

/// 등록된 서비스의 프로필 — 프로세스당 1회 조회 (윈도우는 SCM 질의). 파일이 실제로 있을 때만.
fn registered_service_config() -> Option<&'static PathBuf> {
    static CACHE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| crate::service::registered_config_path().filter(|p| p.is_file()))
        .as_ref()
}

/// `config_path()`가 어느 규칙으로 정해졌는지 — `brv status`가 사용자에게 보여준다.
pub fn profile_source() -> &'static str {
    if PATH_OVERRIDE.get().is_some() {
        "pinned by --config"
    } else if std::env::var_os("BREVDUVA_CONFIG").is_some() {
        "BREVDUVA_CONFIG"
    } else if registered_service_config().is_some() {
        "the registered OS service"
    } else {
        "OS default"
    }
}

/// 원본 파싱용 — 신형(`[[binding]]`)과 구형(톱레벨 channel/agent, [wake]의 dir/policy)을
/// 한 구조로 받아 `normalize`가 신형으로 통일한다.
#[derive(Deserialize)]
struct RawConfig {
    server: String,
    #[serde(rename = "binding", default)]
    bindings: Vec<Binding>,
    wake: Option<RawWake>,
    // ---- 구형 필드 (페이즈 27 이전 단수형) ----
    channel: Option<String>,
    agent: Option<String>,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct RawWake {
    command: String,
    #[serde(default = "default_wake_args")]
    args: Vec<String>,
    #[serde(default = "default_wake_timeout")]
    timeout_s: u64,
    // ---- 구형 필드 — 신형에서는 바인딩 소관 ----
    // (구형 `policy`와 바인딩의 `wake_policy`는 2026-09-03 제거 — "never=저널만"은 처리 없이
    // 소비 확정하는 함정이라 폐기. 파일에 남은 키는 serde가 무시하고, 잠시 깨우기를 멈추는
    // 용도는 `brv daemon pause`가 맡는다)
    dir: Option<String>,
}

impl RawConfig {
    fn normalize(self) -> BrvConfig {
        let mut bindings = self.bindings;
        if bindings.is_empty()
            && let (Some(agent), Some(channel)) = (self.agent, self.channel)
        {
            // 구형 단수 설정 → 바인딩 1개로 해석. wake의 dir/policy도 바인딩으로 이동.
            // org·러너 오버라이드는 구형에 없던 개념 — 미상(None)으로 두면 토큰 폴백이 감당
            bindings.push(Binding {
                org: None,
                agent,
                channel,
                description: self.description,
                wake_dir: self.wake.as_ref().and_then(|w| w.dir.clone()),
                wake_command: None,
                wake_args: None,
            });
        }
        BrvConfig {
            server: self.server,
            wake: self.wake.map(|w| WakeConfig {
                command: w.command,
                args: w.args,
                timeout_s: w.timeout_s,
            }),
            bindings,
        }
    }
}

pub fn load() -> anyhow::Result<BrvConfig> {
    let path = config_path()?;
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("config not found at {path:?} — run `brv init` first"))?;
    parse(&text)
}

/// 파싱+정규화 — load와 테스트가 공유하는 단일 경로.
pub fn parse(text: &str) -> anyhow::Result<BrvConfig> {
    let raw: RawConfig = toml::from_str(text).context("config parse failed")?;
    Ok(raw.normalize())
}

pub fn store(cfg: &BrvConfig) -> anyhow::Result<PathBuf> {
    let path = config_path()?;
    std::fs::create_dir_all(path.parent().expect("config path has parent"))?;
    secure_config_dir();
    std::fs::write(&path, toml::to_string_pretty(cfg)?)?;
    Ok(path)
}

fn keyring_entry(server: &str, token_id: &str) -> anyhow::Result<keyring::Entry> {
    // 계정 키 = 정체성@서버 — 채널은 넣지 않는다 (2026-08-27 수정: 토큰은 org 스코프라
    // 한 에이전트가 여러 채널에 참가). 정체성은 org를 알면 `org/agent`(조직 간 동명
    // 에이전트 충돌 방지, 2026-09-01), 구형(단일 조직 시절)은 `agent`.
    keyring::Entry::new("brevduva", &format!("{token_id}@{server}")).context("keyring unavailable")
}

/// 토큰이 실제로 저장된 위치 — 호출자가 사용자에게 정확히 알려주기 위한 반환값.
#[derive(Debug)]
pub enum TokenStore {
    Keyring,
    File(PathBuf),
}

fn token_file(token_id: &str) -> anyhow::Result<PathBuf> {
    // 파일명에 '/'는 불가 — org 구분자를 '-'로 (org·agent는 케밥 식별자라 안전)
    Ok(config_path()?
        .parent()
        .expect("config has parent")
        .join(format!("token-{}", token_id.replace('/', "-"))))
}

/// 설정 디렉터리를 소유자·SYSTEM·관리자만 접근하도록 좁힌다 (2026-09-03, 사용자 지시).
///
/// 발단은 **윈도우에 권한 처리가 아예 없던 것** — 토큰 파일의 0600은 `#[cfg(unix)]`라
/// 윈도우는 상위 폴더 권한을 그대로 물려받았다. `C:\brevduva` 실측에서 `BUILTIN\Users` 읽기·
/// `Authenticated Users` 수정이 상속돼 같은 PC의 다른 계정이 토큰을 읽고 덮어쓸 수 있었다.
/// 2026-09-03 데몬이 LocalSystem 서비스가 되며 토큰이 자격 증명 관리자(계정별 암호화)에서
/// 평문 파일로 내려왔으므로, 이 구멍은 곧 자격 증명 노출이다.
///
/// 유닉스도 대상이다(0.6.14) — 토큰 파일은 0600이었지만 디렉터리가 0775, 저널이 0664라
/// 바깥 겹(홈 디렉터리 권한)에만 기대고 있었다. 이제 디렉터리 자체를 0700으로 잠근다.
///
/// 토큰만이 아니라 저널(주고받은 메시지 본문)·설정이 같은 폴더에 있어 **디렉터리 단위**로
/// 건다. 프로세스당 1회 — enroll·설정 저장마다 다시 걸 이유가 없다. 실패는 경고만:
/// 권한 조정이 안 된다고 enroll 자체를 막으면 더 나쁘다.
pub fn secure_config_dir() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let result = config_path()
            .map(|p| p.parent().expect("config has parent").to_path_buf())
            .and_then(|dir| restrict_dir(&dir));
        if let Err(e) = result {
            eprintln!(
                "warning: could not restrict permissions on the config directory — other accounts on this machine may be able to read the token and the message journal: {e}"
            );
        }
    });
}

/// 유닉스: 디렉터리 자체를 0700으로. 토큰 파일은 이미 0600이지만 **저널(주고받은 메시지
/// 본문)과 wake 로그는 0664**여서 같은 머신의 다른 계정이 읽을 수 있었다 (2026-09-03 실측:
/// 리눅스 서버의 `~/.config/brevduva`가 775, 저널이 664). 디렉터리에서 막으면 안쪽 파일의
/// 모드와 무관하게 접근 자체가 끊긴다 — 파일마다 모드를 챙기는 것보다 빠뜨릴 구석이 없다.
#[cfg(unix)]
fn restrict_dir(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("chmod 700 {dir:?}"))
}

/// 윈도우: `icacls`로 디렉터리 DACL 재작성 — 보안 설명자 API의 FFI를 늘리지 않는다
/// (service.rs의 `sc sdset`과 같은 판단). 상속을 끊고(`/inheritance:r`) 세 주체만 남긴다.
/// `(OI)(CI)`는 하위 파일·폴더로 상속되므로 이후 만들어지는 토큰 파일이 좁힌 권한을 물려받는다.
/// 소유자는 DACL 쓰기 권한을 늘 가지므로 관리자 승격이 필요 없다.
#[cfg(windows)]
fn restrict_dir(dir: &Path) -> anyhow::Result<()> {
    let sid = user_sid()?;
    let icacls = |target: &Path, args: &[&str]| -> anyhow::Result<()> {
        let out = std::process::Command::new("icacls")
            .arg(target)
            .args(args)
            .output()
            .context("icacls")?;
        anyhow::ensure!(
            out.status.success(),
            "icacls failed: {}",
            String::from_utf8_lossy(&out.stdout).trim()
        );
        Ok(())
    };
    // ① 디렉터리 — 상속을 끊고 세 주체만. `(OI)(CI)`는 앞으로 만들 파일이 물려받게 한다.
    //    사용자 문맥에서만 한다: 서비스(LocalSystem)의 "현재 사용자"는 SYSTEM이라 그대로
    //    좁히면 정작 사람이 자기 설정을 못 읽는다. 규칙은 `brv daemon install`·enroll·설정
    //    저장이 세우고, 서비스는 그 규칙을 따르기만 한다
    if sid != "S-1-5-18" {
        icacls(
            dir,
            &[
                "/inheritance:r",
                "/grant:r",
                "*S-1-5-18:(OI)(CI)F", // SYSTEM — LocalSystem 데몬 서비스가 읽는다
                "*S-1-5-32-544:(OI)(CI)F", // Administrators
                &format!("*{sid}:(OI)(CI)F"), // 소유자
            ],
        )?;
    }
    // ② 이미 있던 파일 — ①만으로는 **예전 권한을 explicit ACE로 굳힌 채 남는다**(상속이
    //    끊기며 그대로 복사됨). 갱신 머신에서는 정작 지금 쓰는 토큰이 노출된 채라 의미가 없다.
    //    ①에 `/T`를 붙이는 방법은 못 쓴다 — `(OI)(CI)`는 파일에 의미가 없어 무시되고, 실측
    //    (2026-09-03) 결과 **파일의 ACE가 전부 사라져 아무도 못 읽는 상태**가 됐다.
    //    그래서 자식만 상속 전용으로 초기화해 ①의 ACE를 물려받게 한다. 소유자 SID가 필요
    //    없는 단계라 **두 문맥 모두** 수행한다 — 서비스가 만든 파일(토큰·상태·로그)은 소유자가
    //    SYSTEM이라 사람이 고칠 수 없고, 사람이 만든 파일은 그 반대다. 서로의 사각을 메운다.
    //    실측(2026-09-03): 관리자 터미널의 install이 만든 토큰 파일은 사용자 문맥에서
    //    "Access is denied"로 남았고, 서비스가 다음 기동에 스스로 고쳤다.
    //    재귀(`/T`)·와일드카드는 쓰지 않는다 (2026-09-04): 이 단계는 SYSTEM 문맥에서도 돌고
    //    디렉터리는 사용자 소유다. `icacls`는 `/L`이 없으면 링크의 **대상**을 조작하므로,
    //    사용자가 안에 심은 정션·심볼릭 링크를 SYSTEM이 따라가 바깥 경로의 ACL을 초기화하는
    //    권한 상승 통로가 된다. 항목을 직접 열거해 재분석 지점(정션·링크·플레이스홀더)은
    //    건너뛰고 일반 파일만 하나씩 `/L`로 처리한다 — 설정 디렉터리는 평면이라 하위 폴더가
    //    없다. 검사와 실행 사이의 교체 창(TOCTOU)은 남지만 재귀가 없어 대상 하위로 번지지
    //    않고, 디렉터리 소유자는 서비스를 설치한 관리자라 그 창은 이미 관리자인 사람에게만 열린다
    use std::os::windows::fs::MetadataExt as _;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    for entry in std::fs::read_dir(dir).with_context(|| format!("list {dir:?}"))? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        // symlink_metadata: 링크를 따라가지 않고 항목 자체를 본다
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            continue;
        }
        // 다른 문맥이 소유한 파일은 여기서 못 고친다 — 그쪽이 자기 기동 때 고친다 (위 설명)
        let _ = icacls(&path, &["/reset", "/L", "/Q"]);
    }
    Ok(())
}

/// 현재 사용자의 SID 문자열 — 내장 `whoami`로 (LookupAccountName FFI 회피).
/// service.rs의 서비스 DACL 부여도 이걸 쓴다.
#[cfg(windows)]
pub(crate) fn user_sid() -> anyhow::Result<String> {
    let out = std::process::Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .context("whoami")?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "DOMAIN\user","S-1-5-21-…" — 마지막 열이 SID
    let sid = text
        .trim()
        .rsplit(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .to_owned();
    anyhow::ensure!(
        sid.starts_with("S-1-"),
        "cannot read the user SID from whoami: {text:?}"
    );
    Ok(sid)
}

/// 토큰 파일 쓰기 — 저장·이전의 공통 출구. 원자적이다 (2026-09-04): 임시 파일에 쓰고
/// (유닉스는 그 시점에 0600) 제자리 rename — 도중에 죽어도 반쯤 쓰인 파일이 `load_token`의
/// "파일 우선" 조회에 잡혀 멀쩡한 키체인 사본을 가리는 일이 없다. 남는 것은 무시되는 `.tmp`뿐.
fn write_token_file(token_id: &str, token: &str) -> anyhow::Result<PathBuf> {
    let path = token_file(token_id)?;
    std::fs::create_dir_all(path.parent().expect("token path has parent"))?;
    // 파일을 만들기 **전에** 좁힌다 — 새 파일이 좁혀진 권한을 상속받게 (윈도우)
    secure_config_dir();
    write_secret_file(&path, token)?;
    Ok(path)
}

/// 비밀 파일의 원자적 교체: `<path>.tmp`에 쓰고 → (유닉스) 0600 → rename. 어느 단계가
/// 실패해도 임시 파일을 치우고 `path`는 손대지 않은 채 남는다.
fn write_secret_file(path: &Path, content: &str) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    let staged = (|| -> anyhow::Result<()> {
        std::fs::write(&tmp, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if staged.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    staged.with_context(|| format!("token file write failed at {path:?}"))
}

/// 이 플랫폼·바이너리에서 OS 키체인을 **주 저장소**로 써도 되는가 — 아니면 토큰 파일이 주다.
///
/// - 맥 (2026-09-01 실측의 근본 대응): 키체인 ACL은 코드 서명 정체성 기준인데, 애드혹 서명
///   (cargo·CI 기본)은 고정 정체성이 없어 **"항상 허용"조차 유지되지 않는다** — 토큰을 읽는
///   모든 표면(데몬 재기동·세션 MCP·훅)에서 프롬프트 폭풍. 그런 빌드는 파일(사용자 개입 0),
///   Developer ID 서명 배포부터는 자동으로 키체인.
/// - 윈도우 (2026-09-03): 데몬이 LocalSystem 서비스라 사용자의 자격 증명 저장소를 못 본다.
/// - 리눅스 (2026-09-04, 종전 `true`를 번복): systemd 사용자 유닛 + `loginctl enable-linger`는
///   **로그인 세션 없이 부팅 때** 데몬을 띄운다. 이 crate의 리눅스 저장소는 D-Bus Secret
///   Service(GNOME Keyring·KWallet)인데 그 컬렉션은 세션이 열고 잠금을 푸는 것이라, 그 시점엔
///   없거나 잠겨 있다 — 키링이 주 저장소면 기동 시점마다 결과가 달라진다. 파일이 주.
fn keychain_is_reliable() -> bool {
    // cfg!(런타임 분기)인 이유: cfg 속성으로 갈랐다가는 맥 전용 본문이 이 윈도우 개발
    // 머신의 검사(clippy·test)를 영영 안 거친다 — 전 플랫폼 컴파일로 검증 사각을 없앤다
    if cfg!(windows) || cfg!(target_os = "linux") {
        return false;
    }
    if !cfg!(target_os = "macos") {
        return true; // 그 밖의 유닉스(BSD 등) — 데몬 서비스 미지원 플랫폼, 종전 동작 유지
    }
    static RELIABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *RELIABLE.get_or_init(|| {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        let Ok(out) = std::process::Command::new("codesign")
            .arg("-dv")
            .arg(&exe)
            .output()
        else {
            return false; // 판별 불가 — 프롬프트 폭풍보다 파일 폴백이 안전한 기본
        };
        let info = String::from_utf8_lossy(&out.stderr);
        out.status.success()
            && !info.contains("Signature=adhoc")
            && info.contains("TeamIdentifier=")
            && !info.contains("TeamIdentifier=not set")
    })
}

/// 토큰 조회 키 후보 — 현행 `org/agent`, 그리고 org를 알 때는 구형 `agent`(단일 조직 시절 저장분).
fn token_ids(binding: &Binding) -> Vec<String> {
    let mut ids = vec![binding.token_id()];
    if binding.org.is_some() {
        ids.push(binding.agent.clone());
    }
    ids
}

/// 토큰 저장 — 주 저장소에 쓴다 (`keychain_is_reliable`: 서명된 맥은 키체인, 그 외는 토큰
/// 파일 0600). 키는 바인딩의 token_id. 키체인이 주인데 쓰기가 실패하면 파일로 폴백.
/// 키체인에 썼으면 **같은 정체성의 토큰 파일은 지운다** (2026-09-04): `load_token`은 파일을
/// 키체인보다 먼저 보고 키체인으로 이전하므로, 옛 파일이 남으면 다음 읽기가 방금 저장한 새
/// 토큰을 옛 것으로 되돌린다. 주 저장소는 하나여야 한다.
pub fn store_token(server: &str, binding: &Binding, token: &str) -> anyhow::Result<TokenStore> {
    let id = binding.token_id();
    if keychain_is_reliable()
        && let Ok(entry) = keyring_entry(server, &id)
        && entry.set_password(token).is_ok()
    {
        for stale in token_ids(binding).iter().filter_map(|c| token_file(c).ok()) {
            if stale.exists()
                && let Err(e) = std::fs::remove_file(&stale)
            {
                eprintln!("warning: could not remove the superseded token file {stale:?}: {e}");
            }
        }
        return Ok(TokenStore::Keyring);
    }
    Ok(TokenStore::File(write_token_file(&id, token)?))
}

/// 파일 → 키체인 이전 (키체인이 주 저장소가 된 시점 — 무서명 빌드에서 서명 빌드로 갱신).
/// 순서: 키체인에 쓰기 → **되읽어 같은 값인지 확인** → 그제야 파일 삭제. 어느 단계가
/// 실패해도 파일은 남는다 — 토큰을 잃는 경로가 없다.
fn migrate_file_to_keychain(
    server: &str,
    id: &str,
    path: &Path,
    token: &str,
) -> anyhow::Result<()> {
    let entry = keyring_entry(server, id)?;
    entry.set_password(token).context("keychain write")?;
    let back = entry.get_password().context("keychain read-back")?;
    anyhow::ensure!(
        back == token,
        "keychain read-back differs from what was written"
    );
    std::fs::remove_file(path).with_context(|| format!("remove {path:?}"))
}

/// 키체인 → 파일 이전 (파일이 주 저장소인 플랫폼에 키체인 시절의 항목이 남은 경우).
/// 같은 순서: 파일 쓰기(0600·디렉터리 좁힘 포함) → 되읽어 확인 → 그제야 키체인 항목 삭제.
/// 삭제 이유는 비밀 사본 최소화와, 파일이 사라졌을 때 옛 항목이 폴백으로 되살아나는 것
/// (stale fallback) 방지다 — 서버에서 회수된 토큰은 어디에 있든 무효이므로 "회수 안 되는
/// 사본"이 이유가 아니다 (2026-09-04 정정). 맥은 지우기도 프롬프트를 띄우는데 프롬프트 회피가
/// 파일 이전의 이유였으므로 항목을 남긴다 (서명 배포로 키체인에 복귀하면 `migrate_file_to_keychain`
/// 이 그 항목을 덮어쓴다).
fn migrate_keychain_to_file(
    server: &str,
    id: &str,
    found_id: &str,
    token: &str,
) -> anyhow::Result<()> {
    let path = write_token_file(id, token)?;
    let back = std::fs::read_to_string(&path).with_context(|| format!("read back {path:?}"))?;
    anyhow::ensure!(
        back.trim() == token.trim(),
        "token file read-back differs from what was written"
    );
    if cfg!(target_os = "macos") {
        return Ok(());
    }
    keyring_entry(server, found_id)?
        .delete_credential()
        .context("keychain entry delete")
}

/// 토큰 로드 — `BREVDUVA_TOKEN` env가 있으면 그것(단일 바인딩 전용 편의 — 에이전트가 여럿이면
/// 전원에게 같은 값이 가므로 부적합). 없으면 주 저장소·보조 저장소를 보되, **보조에서 찾은
/// 토큰은 주 저장소로 이전한다** (2026-09-04, 양방향):
/// - 키체인이 주(서명된 맥): 파일 → (있으면 키체인으로 이전 후 파일 삭제) → 키체인. 파일이
///   먼저인 이유: 무서명 빌드 시절 재enroll로 **파일만 새 토큰**이 됐을 수 있다 — 키체인을
///   먼저 읽으면 옛 토큰으로 접속해 실패한다. 이전이 실패하면 파일을 그대로 두고 그 값을 쓴다.
/// - 파일이 주(윈도우·리눅스·무서명 맥): 파일 → 키체인 → (찾으면 파일로 이전 후 항목 삭제,
///   맥 제외). 무개입 마이그레이션 — 기존 키체인 사용자는 다음 읽기 한 번으로 끝난다.
///
/// 각 저장소 안에서는 org 포함 키 → 구형 agent 키 순. 마지막으로 구형 단일 파일(`token`,
/// 바인딩이 하나일 때만 — 어느 에이전트 것인지 판별 불가). 구형 폴백들 덕에 기존 머신은
/// 재enroll 없이 동작한다. 어느 이전도 **검증 전에는 원본을 지우지 않는다**. 빈 토큰 파일은
/// 토큰이 아니다 — 이전 대상도, 키체인 사본을 가리는 파일도 아니다.
pub fn load_token(cfg: &BrvConfig, binding: &Binding) -> anyhow::Result<String> {
    if let Ok(token) = std::env::var("BREVDUVA_TOKEN") {
        return Ok(token);
    }
    let id = binding.token_id();
    let ids = token_ids(binding);
    // 어느 파일·어느 키로 찾았는지까지 돌려준다 — 이전 후 그 원본을 지우기 위해
    let from_files = || {
        ids.iter().find_map(|c| {
            let path = token_file(c).ok()?;
            let t = std::fs::read_to_string(&path).ok()?.trim().to_owned();
            // 빈 파일은 없는 것으로 — 이전할 "유효한 토큰"이 아니고, 키체인 사본을 가려서도 안 된다
            (!t.is_empty()).then_some((path, t))
        })
    };
    let from_keyring = || {
        ids.iter().find_map(|c| {
            let token = keyring_entry(&cfg.server, c).ok()?.get_password().ok()?;
            Some((c.clone(), token))
        })
    };
    if keychain_is_reliable() {
        if let Some((path, t)) = from_files() {
            if let Err(e) = migrate_file_to_keychain(&cfg.server, &id, &path, &t) {
                eprintln!(
                    "warning: token file {path:?} could not be moved into the keychain (kept as is, will retry next time): {e:#}"
                );
            }
            return Ok(t);
        }
        if let Some((_, t)) = from_keyring() {
            return Ok(t);
        }
    } else {
        if let Some((_, t)) = from_files() {
            return Ok(t);
        }
        if let Some((found_id, t)) = from_keyring() {
            if let Err(e) = migrate_keychain_to_file(&cfg.server, &id, &found_id, &t) {
                eprintln!(
                    "warning: keychain token could not be moved to the token file (kept in the keychain, will retry next time): {e:#}"
                );
            }
            return Ok(t);
        }
    }
    if cfg.bindings.len() <= 1 {
        let legacy = config_path()?
            .parent()
            .expect("config has parent")
            .join("token");
        if let Ok(t) = std::fs::read_to_string(&legacy) {
            return Ok(t.trim().to_owned());
        }
    }
    anyhow::bail!(
        "token for {id:?} not found — run `brv init --enroll`, set BREVDUVA_TOKEN, or place it at {:?}",
        token_file(&id)?
    )
}

/// 전 바인딩의 토큰 일괄 로드 (token_id → token) — 데몬·서비스 기동용.
/// 바인딩이 하나라도 토큰이 없으면 에러 — 설정된 바인딩이 조용히 죽는 것보다 낫다.
pub fn load_tokens(cfg: &BrvConfig) -> anyhow::Result<std::collections::HashMap<String, String>> {
    let mut map = std::collections::HashMap::new();
    for b in &cfg.bindings {
        if let std::collections::hash_map::Entry::Vacant(e) = map.entry(b.token_id()) {
            e.insert(load_token(cfg, b)?);
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-03 (사용자 지시 "토큰 파일은 암호화 되는건가?"): 윈도우 설정 디렉터리는
    /// 소유자·SYSTEM·관리자만 접근할 수 있어야 한다 — 종전에는 상위 폴더의 `Users` 읽기·
    /// `Authenticated Users` 수정이 상속돼 같은 PC의 다른 계정이 평문 토큰을 읽을 수 있었다.
    /// 2026-09-03 (같은 발단): 유닉스는 디렉터리 자체를 0700으로 — 토큰(0600)은 이미
    /// 안전했지만 저널(0664, 메시지 본문)이 같은 머신의 다른 계정에게 열려 있었다.
    #[cfg(unix)]
    #[test]
    fn config_dir_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("brv-mode-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o775)).expect("loosen");
        restrict_dir(&dir).expect("restrict");
        let mode = std::fs::metadata(&dir).expect("stat").permissions().mode() & 0o777;
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(
            mode, 0o700,
            "group and others must not reach the config dir"
        );
    }

    #[cfg(windows)]
    fn acl_of(p: &Path) -> String {
        let out = std::process::Command::new("icacls")
            .arg(p)
            .output()
            .expect("icacls");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[cfg(windows)]
    #[test]
    fn config_dir_is_locked_to_the_owner_on_windows() {
        let dir = std::env::temp_dir().join(format!("brv-acl-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        // 좁히기 **전에** 만들어 둔 파일 — 갱신 머신에 이미 있던 토큰 파일에 해당한다
        let existing = dir.join("token-existing");
        std::fs::write(&existing, "brv_x").expect("token file");
        restrict_dir(&dir).expect("restrict");
        let (dir_acl, file_acl) = (acl_of(&dir), acl_of(&existing));
        std::fs::remove_dir_all(&dir).ok();
        // 그룹 이름은 OS 언어에 따라 달라지므로 **주체 수**로 검사한다 — 딱 셋만 남아야 한다
        // (SYSTEM·Administrators·소유자). 넷째가 생기면 Users류가 되살아난 것
        let principals = |acl: &str| acl.lines().filter(|l| l.contains(":(")).count();
        assert_eq!(principals(&dir_acl), 3, "directory: {dir_acl:?}");
        assert_eq!(
            principals(&file_acl),
            3,
            "a file that predates the fix must be narrowed too: {file_acl:?}"
        );
        // 소유자 계정은 로컬라이즈되지 않는다 — 자기 자신이 남았는지는 이름으로 확인
        let me = std::process::Command::new("whoami")
            .output()
            .expect("whoami");
        let me = String::from_utf8_lossy(&me.stdout).trim().to_lowercase();
        assert!(
            file_acl.to_lowercase().contains(&me),
            "the owner must keep access: {file_acl:?}"
        );
    }

    /// 2026-09-04: 자식 권한 초기화는 SYSTEM 문맥에서도 도는데 디렉터리는 사용자 소유다 —
    /// 사용자가 심어 둔 정션을 따라가 바깥 경로의 ACL을 건드리면 권한 상승이다. 정션(재분석
    /// 지점)은 건너뛰고 그 대상은 손대지 않아야 한다. 대상 파일에 미리 explicit ACE를 줘
    /// 두어, 따라갔다면(`/reset` = 상속 복원) 주체 수가 달라져 드러나게 한다.
    #[cfg(windows)]
    #[test]
    fn child_reset_does_not_follow_junctions_on_windows() {
        let base = std::env::temp_dir().join(format!("brv-junction-test-{}", std::process::id()));
        let (dir, outside) = (base.join("config"), base.join("outside"));
        std::fs::create_dir_all(&dir).expect("config dir");
        std::fs::create_dir_all(&outside).expect("outside dir");
        let victim = outside.join("victim.txt");
        std::fs::write(&victim, "keep my acl").expect("victim");
        let sid = user_sid().expect("sid");
        let out = std::process::Command::new("icacls")
            .arg(&victim)
            .args(["/inheritance:r", "/grant:r", &format!("*{sid}:F"), "/Q"])
            .output()
            .expect("icacls");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        // 정션은 특권 없이 만들 수 있다 (심볼릭 링크와 달리) — 공격자 모델에 맞는 쪽
        let out = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(dir.join("planted"))
            .arg(&outside)
            .output()
            .expect("mklink");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let before = acl_of(&victim);
        restrict_dir(&dir).expect("restrict");
        let after = acl_of(&victim);
        std::fs::remove_dir_all(&base).ok();
        let principals = |acl: &str| acl.lines().filter(|l| l.contains(":(")).count();
        assert_eq!(principals(&before), 1, "setup: {before:?}");
        assert_eq!(
            before, after,
            "a file behind a planted junction must be untouched"
        );
    }

    /// 2026-09-04: 토큰 파일 쓰기는 원자적이어야 한다 — 임시 파일 경유·제자리 교체, 임시 파일
    /// 잔존 없음, 유닉스 0600. 반쯤 쓰인 파일이 "파일 우선" 조회에 잡히면 키체인 사본을 가린다.
    #[test]
    fn secret_file_write_is_atomic_and_private() {
        let dir = std::env::temp_dir().join(format!("brv-secret-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("token-x");
        std::fs::write(&path, "old").expect("seed");
        write_secret_file(&path, "brv_new").expect("write");
        let content = std::fs::read_to_string(&path).expect("read");
        let leftover = path.with_extension("tmp").exists();
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777
        };
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(content, "brv_new", "replaces the old content in place");
        assert!(!leftover, "no staging file may remain");
        #[cfg(unix)]
        assert_eq!(mode, 0o600);
    }

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

    fn binding(agent: &str, channel: &str) -> Binding {
        Binding {
            org: None,
            agent: agent.into(),
            channel: channel.into(),
            description: String::new(),
            wake_dir: None,
            wake_command: None,
            wake_args: None,
        }
    }

    #[test]
    fn multi_binding_config_survives_toml_roundtrip() {
        // 신형: 전역 [wake] + [[binding]] 배열(org·러너 오버라이드 포함)이 왕복 보존 (페이즈 27)
        let cfg = BrvConfig {
            server: "https://api.brevduva.dev".into(),
            wake: Some(WakeConfig {
                command: "/home/user/.local/bin/claude".into(),
                args: wake_preset_args("full").unwrap(),
                timeout_s: 900,
            }),
            bindings: vec![
                Binding {
                    org: Some("acme".into()),
                    wake_dir: Some("/home/user/app".into()),
                    ..binding("backend", "saju-engine")
                },
                Binding {
                    wake_command: Some("/usr/bin/codex".into()),
                    wake_args: Some(vec!["exec".into(), "{prompt}".into()]),
                    ..binding("docs", "myapp")
                },
            ],
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back = parse(&text).unwrap();
        assert_eq!(back.bindings.len(), 2);
        let wake = back.wake.unwrap();
        assert_eq!(wake_preset_of(&wake.args), Some("full"));
        assert_eq!(wake.timeout_s, 900);
        assert_eq!(back.bindings[0].org.as_deref(), Some("acme"));
        assert_eq!(back.bindings[0].full_label(), "acme/backend@saju-engine");
        assert_eq!(back.bindings[0].token_id(), "acme/backend");
        assert_eq!(back.bindings[0].wake_dir.as_deref(), Some("/home/user/app"));

        assert_eq!(
            back.bindings[1].wake_command.as_deref(),
            Some("/usr/bin/codex")
        );
        assert_eq!(back.bindings[1].token_id(), "docs");
    }

    #[test]
    fn legacy_single_config_reads_as_one_binding() {
        // 구형(페이즈 27 이전): 톱레벨 channel/agent + [wake]의 dir/policy →
        // 바인딩 1개로 해석되고 dir/policy가 바인딩으로 이동한다 (기존 머신 재설정 불요)
        let cfg = parse(
            r#"
            server = "https://api.brevduva.dev"
            channel = "myapp"
            agent = "backend"
            description = "백엔드"
            [wake]
            policy = "never"  # 2026-09-03 제거된 구형 키 — 무시되어야 한다 (파싱 실패 금지)
            command = "claude"
            dir = "C:\\test-backend"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.bindings.len(), 1);
        let b = &cfg.bindings[0];
        assert_eq!((b.agent.as_str(), b.channel.as_str()), ("backend", "myapp"));
        assert_eq!(b.description, "백엔드");
        assert_eq!(b.wake_dir.as_deref(), Some("C:\\test-backend"));
        let wake = cfg.wake.as_ref().unwrap();
        assert_eq!(wake.command, "claude");
        assert_eq!(wake.timeout_s, 600);
        assert!(wake.args.iter().any(|a| a.contains("{prompt}")));
        // 재저장은 신형으로 — 구형 필드가 남지 않는다
        let stored =
            toml::to_string_pretty(&parse(&toml::to_string_pretty(&cfg).unwrap()).unwrap());
        assert!(!stored.unwrap().contains("policy = \"never\"\ncommand"));
    }

    #[test]
    fn select_and_find_resolve_bindings() {
        let mut cfg = BrvConfig {
            server: "http://127.0.0.1:8080".into(),
            wake: None,
            bindings: vec![binding("backend", "saju-engine")],
        };
        // 단일 바인딩: 무지정 선택 OK
        assert_eq!(cfg.select(None).unwrap().channel, "saju-engine");
        cfg.bindings.push(binding("docs", "myapp"));
        cfg.bindings.push(binding("backend", "myapp"));
        // 복수 바인딩: 무지정은 에러 (조용한 오발신 방지)
        assert!(cfg.select(None).is_err());
        // agent@channel 정확 일치
        assert_eq!(cfg.select(Some("backend@myapp")).unwrap().channel, "myapp");
        // 에이전트 단독: 유일하면 통과, 중복이면 에러
        assert_eq!(cfg.find("docs").unwrap().channel, "myapp");
        assert!(cfg.find("backend").is_err());
        assert!(cfg.find("ghost").is_err());
    }

    #[test]
    fn org_disambiguates_same_named_bindings() {
        // 2026-09-01 보강: 다른 조직의 동명 agent@channel은 org 접두로만 구분된다
        let cfg = BrvConfig {
            server: "http://127.0.0.1:8080".into(),
            wake: None,
            bindings: vec![
                Binding {
                    org: Some("acme".into()),
                    ..binding("backend", "myapp")
                },
                Binding {
                    org: Some("personal".into()),
                    ..binding("backend", "myapp")
                },
            ],
        };
        // org 없는 선택자는 애매 — 완전 표기 요구
        assert!(cfg.find("backend@myapp").is_err());
        assert!(cfg.find("backend").is_err());
        // org 접두로 유일 결정
        assert_eq!(
            cfg.find("acme/backend@myapp").unwrap().org.as_deref(),
            Some("acme")
        );
        assert_eq!(
            cfg.find("personal/backend").unwrap().org.as_deref(),
            Some("personal")
        );
        // 토큰 키도 org로 갈라진다 (키체인 덮어쓰기 방지의 핵심)
        assert_ne!(cfg.bindings[0].token_id(), cfg.bindings[1].token_id());
    }

    #[test]
    fn upsert_preserves_operational_settings() {
        // 재enroll(같은 agent@channel)이 wake_dir·러너 오버라이드를 지우면 안 된다
        let mut cfg = BrvConfig {
            server: "http://127.0.0.1:8080".into(),
            wake: None,
            bindings: vec![Binding {
                wake_dir: Some("/app".into()),
                wake_command: Some("/usr/bin/codex".into()),
                description: "old".into(),
                ..binding("backend", "myapp")
            }],
        };
        // 구형(org 없음) 바인딩에 org 있는 enroll이 오면 — 교체 + org 채움
        let replaced = cfg.upsert_binding(Binding {
            org: Some("personal".into()),
            description: "new".into(),
            ..binding("backend", "myapp")
        });
        assert!(replaced);
        assert_eq!(cfg.bindings.len(), 1);
        let b = &cfg.bindings[0];
        assert_eq!(b.org.as_deref(), Some("personal"));
        assert_eq!(b.description, "new");
        assert_eq!(b.wake_dir.as_deref(), Some("/app"));
        assert_eq!(b.wake_command.as_deref(), Some("/usr/bin/codex"));
        // 같은 이름이라도 다른 org면 별개 바인딩으로 추가
        assert!(!cfg.upsert_binding(Binding {
            org: Some("acme".into()),
            ..binding("backend", "myapp")
        }));
        assert_eq!(cfg.bindings.len(), 2);
        // 새 (agent, channel)도 추가
        assert!(!cfg.upsert_binding(binding("backend", "other")));
        assert_eq!(cfg.bindings.len(), 3);
    }
}
