// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! macOS 앱 묶음(`Brevduva.app`) 설치 형태의 서비스 등록 (2026-09-22).
//!
//! 발단(2026-09-21 맥북 실측): Developer ID로 서명한 단독 실행 파일을 `~/Library/LaunchAgents`의
//! plist로 등록하면 시스템 설정의 백그라운드 항목에 프로그램 이름이 아니라 **인증서의 개발자
//! 이름**이 나온다. brv를 앱 묶음 안에 넣어도, plist에 `AssociatedBundleIdentifiers`를 넣어도,
//! 앱을 `~/Applications`로 옮겨도 최종 표시는 개발자 이름이었다. 앱 이름("Brevduva")으로 나오는
//! 것은 앱 묶음 안 `Contents/Library/LaunchAgents`의 plist를 **SMAppService**로 등록했을 때뿐이다
//! (Apple이 macOS 13부터 plist 설치의 대체로 제공하는 정식 경로).
//!
//! SMAppService는 Swift·Objective-C API라서 묶음 안의 작은 도구(`brevduva-service`,
//! packaging/macos/service.swift)가 호출하고, brv는 그 도구를 실행해 결과를 읽는다.
//!
//! 묶음 안의 plist는 서명으로 봉인돼 **사용자별 값을 담을 수 없다** — 옛 방식이 plist에 굽던
//! 설치자 PATH·`BREVDUVA_CONFIG`·로그 경로는 설정 폴더의 표지 파일([`ServiceEnv`])에 적어 두고,
//! launchd가 띄운 데몬이 기동 직후 그 값을 입고 자기 자신을 다시 실행한다([`adopt_service_env`]).
//!
//! 이 파일은 모든 OS에서 컴파일된다 (config.rs `keychain_is_reliable`과 같은 이유 — 맥 전용으로
//! 가르면 이 윈도우 개발 머신의 clippy·test가 영영 못 본다). 맥이 아니면 실행 시점에 물러난다.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// launchd 서비스 이름 — 옛 방식 plist와 묶음 안 plist가 같은 값을 쓴다 (둘이 동시에 등록되면 안 된다).
pub const LAUNCHD_LABEL: &str = "dev.brevduva.brv-daemon";
/// 묶음 안 등록 도구의 파일 이름 (`Contents/MacOS/` 아래, brv 옆).
const HELPER_NAME: &str = "brevduva-service";
/// SMAppService는 macOS 13부터다 — 그 아래에서는 옛 방식 plist로 등록한다.
const MIN_MACOS_MAJOR: u32 = 13;
/// 표지 파일을 입고 다시 실행된 데몬임을 알리는 환경변수 — 다시 실행이 되풀이되지 않게 한다.
const ADOPTED_ENV: &str = "BRV_SERVICE_ENV_ADOPTED";

/// 등록 표지 — SMAppService로 등록했다는 사실과, 봉인된 plist에 담지 못한 사용자별 값.
/// 위치는 프로필과 무관한 고정 경로(`~/Library/Application Support/brevduva/launchd-service.toml`):
/// 어느 프로필을 쓸지(`config`)를 이 파일이 알려 주므로 프로필 경로에 기대면 순환이다.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceEnv {
    /// 설치자 셸의 PATH — launchd 기본 PATH에는 사용자 설치 경로가 없어 wake 명령을 못 찾는다
    /// (2026-08-29 실사고, 옛 방식은 plist의 EnvironmentVariables에 구웠다).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `brv daemon install --config`의 절대 경로. None = 기본 프로필.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
}

/// `<…>/<이름>.app/Contents/MacOS/<파일>` 꼴이면 그 `.app` 경로. 심볼릭 링크를 푼 경로를 넘길 것.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let is = |p: &Path, name: &str| p.file_name().is_some_and(|n| n == name);
    (is(macos, "MacOS")
        && is(contents, "Contents")
        && app
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("app")))
    .then(|| app.to_path_buf())
}

fn marker_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("brevduva")
            .join("launchd-service.toml"),
    )
}

/// 표지 파일 읽기. 없거나 못 읽으면 None — SMAppService 등록이 없는 것으로 본다.
pub fn read_marker() -> Option<ServiceEnv> {
    toml::from_str(&std::fs::read_to_string(marker_path()?).ok()?).ok()
}

fn write_marker(env: &ServiceEnv) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let path = marker_path().context("cannot resolve the config directory")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, toml::to_string(env)?).with_context(|| format!("write {path:?}"))
}

fn remove_marker() {
    if let Some(path) = marker_path()
        && path.exists()
        && let Err(e) = std::fs::remove_file(&path)
    {
        eprintln!("warning: could not remove {path:?}: {e}");
    }
}

/// 옛 방식 plist의 `EnvironmentVariables` 값 — 새 방식으로 옮길 때 표지 파일로 이어받는다.
/// 옛 설치는 `&`·`<`를 XML 이스케이프해 썼다 (service.rs `install_legacy`).
pub fn env_from_plist(text: &str, key: &str) -> Option<String> {
    let after = text.split(&format!("<key>{key}</key>")).nth(1)?;
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")? + start;
    Some(after[start..end].replace("&lt;", "<").replace("&amp;", "&"))
}

/// 등록 도구가 알려 준 SMAppService 상태.
#[derive(Debug, PartialEq, Eq)]
pub enum HelperStatus {
    /// 등록됐고 launchd가 띄운다.
    Enabled,
    /// 등록됐지만 사용자가 시스템 설정의 백그라운드 항목에서 허용해야 뜬다.
    RequiresApproval,
    /// 등록되지 않았다.
    NotRegistered,
    /// 묶음 안에서 서비스 정의를 찾지 못했다 (등록 전이거나 묶음이 온전하지 않다).
    NotFound,
    /// 도구가 상태 줄을 내지 않았다 — 실행 실패 등. 도구의 출력을 담는다.
    Unknown(String),
}

/// 도구 출력의 마지막 `after: <상태>` 줄을 읽는다 (service.swift가 register·unregister·status 뒤에 항상 찍는다).
pub fn parse_helper_status(output: &str) -> HelperStatus {
    match output
        .lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix("after:"))
        .map(str::trim)
    {
        Some("enabled") => HelperStatus::Enabled,
        Some("requiresApproval") => HelperStatus::RequiresApproval,
        Some("notRegistered") => HelperStatus::NotRegistered,
        Some("notFound") => HelperStatus::NotFound,
        _ => HelperStatus::Unknown(output.trim().to_owned()),
    }
}

/// `sw_vers -productVersion` 출력("26.6.2")의 주 버전.
pub fn macos_major(product_version: &str) -> Option<u32> {
    product_version.trim().split('.').next()?.parse().ok()
}

/// 이 실행 파일이 SMAppService로 등록할 수 있는 앱 묶음 안에 있으면 그 등록 도구의 경로.
/// 조건: 맥 · 심볼릭 링크를 푼 실행 파일이 `*.app/Contents/MacOS/` 아래 · 옆에 도구가 있음 · macOS 13 이상.
/// (`~/.local/bin/brv`는 묶음 안을 가리키는 링크라서 링크를 풀어야 한다.)
pub fn bundled_helper() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let exe = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    bundle_of(&exe)?;
    let helper = exe.parent()?.join(HELPER_NAME);
    if !helper.is_file() {
        return None;
    }
    let out = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    (macos_major(&String::from_utf8_lossy(&out.stdout))? >= MIN_MACOS_MAJOR).then_some(helper)
}

fn run_helper(helper: &Path, command: &str) -> HelperStatus {
    match std::process::Command::new(helper)
        .arg(command)
        .stdin(std::process::Stdio::null())
        .output()
    {
        Ok(out) => parse_helper_status(&String::from_utf8_lossy(&out.stdout)),
        Err(e) => HelperStatus::Unknown(format!("could not run {}: {e}", helper.display())),
    }
}

/// SMAppService 등록. `env`는 표지 파일에 적는다 — 등록보다 먼저: 등록 직후 launchd가 데몬을 띄우고
/// 데몬은 기동하자마자 표지를 읽는다. 이미 등록돼 있어도 된다(도구가 실패를 내도 상태가 enabled면 성공).
pub fn register(helper: &Path, env: &ServiceEnv) -> anyhow::Result<()> {
    write_marker(env)?;
    match run_helper(helper, "register") {
        HelperStatus::Enabled => Ok(()),
        HelperStatus::RequiresApproval => {
            println!(
                "registered, but macOS is waiting for your approval: System Settings → General → Login Items & Extensions → allow \"Brevduva\" under App Background Activity. The receiver starts once it is allowed."
            );
            Ok(())
        }
        other => {
            remove_marker();
            anyhow::bail!("macOS did not register the background service: {other:?}")
        }
    }
}

/// SMAppService 등록 해제 + 표지 제거. 등록이 없었어도 오류가 아니다.
pub fn unregister(helper: &Path) {
    if let HelperStatus::Unknown(detail) = run_helper(helper, "unregister") {
        eprintln!("warning: the background service may still be registered: {detail}");
    }
    remove_marker();
}

/// 등록 상태 조회.
pub fn status(helper: &Path) -> HelperStatus {
    run_helper(helper, "status")
}

/// launchd가 SMAppService 등록으로 띄운 데몬이면, 표지 파일의 값을 입고 자기 자신을 다시 실행한다.
///
/// 묶음 안 plist는 봉인돼 있어 PATH·`BREVDUVA_CONFIG`·로그 경로를 담지 못한다. 다시 실행(exec)은 같은
/// pid로 프로세스 이미지만 바꾸므로 launchd의 감독(KeepAlive)이 그대로 이어지고, 환경을 스레드가
/// 생기기 전에 정할 수 있다 (에디션 2024의 unsafe `set_var`를 쓰지 않는다 — main.rs의 윈도우 서비스
/// 모드와 같은 원칙). 로그는 옛 방식과 같은 `~/Library/Logs/brv-daemon.log`.
///
/// 해당하지 않으면(맥이 아님 · launchd가 띄운 것이 아님 · 표지 없음 · 이미 입었음) 그냥 돌아온다.
/// 다시 실행이 실패해도 돌아온다 — 데몬은 기본 환경으로라도 떠야 한다.
pub fn adopt_service_env() {
    if !cfg!(target_os = "macos")
        || std::env::var("XPC_SERVICE_NAME").as_deref() != Ok(LAUNCHD_LABEL)
        || std::env::var_os(ADOPTED_ENV).is_some()
    {
        return;
    }
    let Some(env) = read_marker() else {
        return; // 옛 방식 plist가 띄운 데몬 — 환경·로그는 plist가 이미 줬다
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let (Ok(exe), Some(home)) = (std::env::current_exe(), dirs::home_dir()) else {
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.args(std::env::args_os().skip(1)).env(ADOPTED_ENV, "1");
        if let Some(path) = &env.path {
            cmd.env("PATH", path);
        }
        if let Some(config) = &env.config {
            cmd.env("BREVDUVA_CONFIG", config);
        }
        let logs = home.join("Library/Logs");
        let _ = std::fs::create_dir_all(&logs);
        if let Ok(log) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(logs.join("brv-daemon.log"))
            && let Ok(log2) = log.try_clone()
        {
            cmd.stdout(log).stderr(log2);
        }
        // 성공하면 돌아오지 않는다
        let err = cmd.exec();
        eprintln!("warning: could not restart with the service environment: {err}");
    }
    #[cfg(not(unix))]
    let _ = env;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_is_recognised_only_in_the_app_layout() {
        assert_eq!(
            bundle_of(Path::new(
                "/Users/u/Library/Application Support/brevduva/Brevduva.app/Contents/MacOS/brv"
            )),
            Some(PathBuf::from(
                "/Users/u/Library/Application Support/brevduva/Brevduva.app"
            ))
        );
        // 단독 설치·개발 빌드·이름만 비슷한 경로는 묶음이 아니다
        assert_eq!(bundle_of(Path::new("/Users/u/.local/bin/brv")), None);
        assert_eq!(bundle_of(Path::new("/src/target/release/brv")), None);
        assert_eq!(
            bundle_of(Path::new("/x/Brevduva.app/Contents/Resources/brv")),
            None
        );
        assert_eq!(bundle_of(Path::new("/x/Brevduva/Contents/MacOS/brv")), None);
    }

    #[test]
    fn helper_status_is_read_from_the_last_after_line() {
        // 2026-09-21 맥북 실측 출력 그대로
        let registered = "bundle: /Users/u/Library/Application Support/brevduva/Brevduva.app\nbefore: notFound\nregister: ok\nafter: enabled\n";
        assert_eq!(parse_helper_status(registered), HelperStatus::Enabled);
        // 이미 등록된 상태에서 다시 등록하면 도구는 실패를 찍지만 상태는 enabled다 — 성공으로 읽어야 한다
        let again = "before: enabled\nregister: failed — Error Domain=SMAppServiceErrorDomain Code=1\nafter: enabled\n";
        assert_eq!(parse_helper_status(again), HelperStatus::Enabled);
        assert_eq!(
            parse_helper_status("before: notRegistered\nregister: ok\nafter: requiresApproval\n"),
            HelperStatus::RequiresApproval
        );
        assert_eq!(
            parse_helper_status("before: enabled\nunregister: ok\nafter: notRegistered\n"),
            HelperStatus::NotRegistered
        );
        assert!(matches!(
            parse_helper_status("dyld: Library not loaded"),
            HelperStatus::Unknown(_)
        ));
    }

    #[test]
    fn legacy_plist_environment_carries_over() {
        // 옛 install이 쓰던 plist 모양 — PATH의 &는 이스케이프돼 있다
        let plist = r#"<key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>/Users/u/.local/bin:/opt/a&amp;b/bin:/usr/bin</string>
    <key>BREVDUVA_CONFIG</key><string>/Users/u/profiles/work.toml</string>
  </dict>"#;
        assert_eq!(
            env_from_plist(plist, "PATH").as_deref(),
            Some("/Users/u/.local/bin:/opt/a&b/bin:/usr/bin")
        );
        assert_eq!(
            env_from_plist(plist, "BREVDUVA_CONFIG").as_deref(),
            Some("/Users/u/profiles/work.toml")
        );
        assert_eq!(env_from_plist("<dict/>", "PATH"), None);
    }

    #[test]
    fn marker_round_trips_and_omits_absent_values() {
        let env = ServiceEnv {
            path: Some("/Users/u/.local/bin:/usr/bin".into()),
            config: None,
        };
        let text = toml::to_string(&env).unwrap();
        assert!(!text.contains("config"));
        assert_eq!(toml::from_str::<ServiceEnv>(&text).unwrap(), env);
        // 빈 파일도 "등록됨·기본 프로필·PATH 없음"으로 읽힌다
        assert_eq!(
            toml::from_str::<ServiceEnv>("").unwrap(),
            ServiceEnv::default()
        );
    }

    #[test]
    fn macos_major_version_is_parsed() {
        assert_eq!(macos_major("26.6.2\n"), Some(26));
        assert_eq!(macos_major("12.7.6"), Some(12));
        assert_eq!(macos_major(""), None);
    }
}
