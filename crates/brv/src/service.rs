// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! OS 서비스 등록·해제 — `brv daemon install` / `uninstall` (페이즈 7).
//!
//! 공통 설계: **깨우기는 사용자 컨텍스트**에서 일어나야 한다 — 토큰·wake용 CLI 로그인
//! (claude 등)·프로젝트가 사용자 프로필 소속이라서. 머신당 서비스 1개(이름 고정), 프로필
//! 선택은 `--config`로.
//!
//! - Linux: systemd 사용자 유닛 + `loginctl enable-linger`(부팅 시 로그인 없이 기동)
//! - macOS: LaunchAgent (로그인 시 기동 + KeepAlive). 등록 길은 둘 (2026-09-22): brv가 앱 묶음
//!   `Brevduva.app` 안에서 실행되면 **SMAppService**(묶음 안 plist — 시스템 설정에 "Brevduva"로
//!   표시된다), 단독 실행 파일이거나 macOS 13 미만이면 옛 방식(`~/Library/LaunchAgents`의 plist —
//!   서명 개발자 이름으로 표시된다). 근거와 실측은 macos_bundle.rs
//! - Windows: SCM 진짜 서비스, **LocalSystem** (2026-09-03 사용자 확정 "리시버는 전달자일 뿐" —
//!   종전의 사용자 계정 서비스는 암호 입력이 필수라 무암호 상주가 불가능했다). 듣기는 시스템
//!   계정이, 깨우기는 로그온한 사용자의 토큰을 빌려 그 사용자 명의로 (winspawn.rs — 백신·
//!   업데이트 에이전트와 같은 구조). 설치는 관리자 터미널 1회, 이후 재기동은 소유자 권한으로
//!   (서비스 DACL 부여). 토큰은 파일 저장 — 시스템 계정은 사용자 자격 증명 저장소를 못 본다

pub const SERVICE_NAME: &str = "brv-daemon";

/// 등록된 OS 서비스가 있는가 — `init --enroll`이 "이미 무인 셋업이 끝난 머신"을 판별하는 데 쓴다.
pub fn registered() -> bool {
    registration().is_some()
}

/// 등록된 OS 서비스가 쓰는 설정 파일 (2026-09-04, 온보딩 재설계 2) — `config::config_path()`의
/// 폴백. 실측 발단: 서비스는 `C:\brevduva\config.toml`로 등록돼 있는데 사용자 셸의 `brv status`가
/// 기본 경로(`%APPDATA%`)의 옛 설정을 봐 "토큰 없음·데몬 없음"으로 헤맸다. 서비스가 있는 머신의
/// "이 머신 프로필"은 서비스의 것이다. None = 미등록이거나 기본 경로를 쓰는 서비스.
pub fn registered_config_path() -> Option<std::path::PathBuf> {
    registration()?.map(std::path::PathBuf::from)
}

/// 갱신 잔재 정리 (2026-09-12, RECEIVER_REBUILD_PLAN 10단계·P8) — 실행 중인 파일은 지울 수 없어 갱신은 옛 파일을
/// 비켜 둔다(서비스 [`align_binary`]의 `brv.old`, 윈도우 설치기의 `brv.exe.old`·`brv.exe.old.<id>`). 이 실행 파일
/// 옆의 그런 파일을 지우고 지운 것을 돌려준다. 아직 옛 프로세스(러너가 쥔 옛 `brv mcp` 등)가 쓰고 있으면 지우지
/// 못하고 다음 기회(리시버 기동·`brv daemon restart`)에 다시 한다. 종전에는 다음 갱신 때에야 덮어썼다.
pub fn sweep_parked_binaries(exe: &std::path::Path) -> Vec<std::path::PathBuf> {
    let (Some(dir), Some(name), Some(stem)) = (
        exe.parent(),
        exe.file_name().and_then(|n| n.to_str()),
        exe.file_stem().and_then(|n| n.to_str()),
    ) else {
        return Vec::new();
    };
    let parked_by_service = format!("{stem}.old");
    let service_variant = format!("{parked_by_service}.");
    let parked_by_installer = format!("{name}.old");
    let installer_variant = format!("{parked_by_installer}.");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|file| {
                    file == parked_by_service
                        || file == parked_by_installer
                        || file.starts_with(&service_variant)
                        || file.starts_with(&installer_variant)
                })
        })
        .filter(|path| std::fs::remove_file(path).is_ok())
        .collect()
}

/// 러너 등록이 어느 brv 버전의 형식으로 쓰였는지 적어 두는 표시 파일 — 설정 파일 옆 `mcp-registered.version`.
/// 갱신 뒤 사용자가 `brv mcp register`를 손으로 다시 치지 않게 하기 위한 것(2026-09-13, P8): 0.7.0 갱신 직후
/// Codex가 옛 등록(`--binding`)으로 중계기를 띄워 "MCP startup failed"만 보인 실사고 — 등록 형식이 바뀌는 갱신은
/// 제품이 스스로 등록을 새로 써야 한다. 표시 파일의 버전이 지금 실행 파일과 다르면(없으면) 갱신이 있었던 것이다.
pub fn registration_stamp_path(config_path: &std::path::Path) -> std::path::PathBuf {
    config_path
        .parent()
        .map(|dir| dir.join("mcp-registered.version"))
        .unwrap_or_else(|| std::path::PathBuf::from("mcp-registered.version"))
}

/// 러너 등록을 지금 버전 형식으로 다시 써야 하는가 — 표시 파일이 없거나 다른 버전이다.
pub fn registrations_stale(config_path: &std::path::Path) -> bool {
    std::fs::read_to_string(registration_stamp_path(config_path))
        .map(|v| v.trim() != env!("CARGO_PKG_VERSION"))
        .unwrap_or(true)
}

/// 러너 등록을 지금 버전 형식으로 썼다고 표시한다.
pub fn stamp_registrations(config_path: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(
        registration_stamp_path(config_path),
        concat!(
            env!("CARGO_PKG_VERSION"),
            "
"
        ),
    )
}

/// 서비스가 실제로 실행하는 바이너리 — 갱신이 이 파일에 닿아야 새 코드가 뜬다 (`align_binary`).
pub fn registered_exe() -> Option<std::path::PathBuf> {
    registration_exe()
}

/// 갱신 설치가 서비스에 닿게 한다 (2026-09-04, 사용자 지적 "왜 사용자가 파일을 복사해야 하나").
///
/// 설치기는 CLI 경로(`~/.local/bin`)만 바꾸고 `brv daemon restart`를 부른다. 그런데 서비스가
/// **다른 경로로 등록돼 있으면**(이 윈도우 머신 실측: 서비스는 `C:\brevduva\brv.exe`, 설치기는
/// `%USERPROFILE%\.local\bin\brv.exe`) 재기동해도 옛 바이너리가 다시 뜬다 — 갱신했다고 믿는데
/// 코드는 그대로인 **조용한 실패**다. 그래서 재기동 전에 서비스 쪽 파일을 지금 이 바이너리로
/// 맞춘다: 실행 중인 파일은 지우지 못하므로 이름을 바꿔 비켜 두고(두 OS 모두 허용) 복사한다.
///
/// 버전이 같으면 아무것도 하지 않는다 — 설정 변경마다 파일을 건드리지 않기 위해. 실패는
/// 치명적이지 않다(경고 후 재기동은 그대로 진행) — 다만 그때는 사용자가 알아야 한다.
/// 반환: `Some((경로, 옛 버전))` = 교체함, `None` = 할 일 없음·판단 불가.
pub fn align_binary() -> Option<(std::path::PathBuf, String)> {
    let current = std::env::current_exe().ok()?;
    let registered = registered_exe()?;
    // 같은 파일이면 설치기가 이미 덮었다 (유닉스의 보통 경우)
    if same_file(&current, &registered) {
        return None;
    }
    let installed_version = probe_version(&registered)?;
    let ours = format!("brv {}", env!("CARGO_PKG_VERSION"));
    if installed_version.trim() == ours {
        return None;
    }
    let parked = match park_aside(&registered) {
        Ok(parked) => parked,
        Err(e) => {
            eprintln!(
                "warning: the service runs {} but this is {ours} — could not move the old file aside ({e}). The daemon keeps running the old version — run `brv daemon restart` again once the old bridges have exited.",
                registered.display()
            );
            return None;
        }
    };
    if let Err(e) = std::fs::copy(&current, &registered) {
        let _ = std::fs::rename(&parked, &registered); // 되돌린다 — 반쪽 상태로 두지 않는다
        eprintln!(
            "warning: could not update the service binary at {} ({e}) — the daemon keeps the old version",
            registered.display()
        );
        return None;
    }
    Some((registered, installed_version.trim().to_owned()))
}

/// 실행 중인 서비스 파일을 비켜 둔다 — `brv.old`가 비어 있거나 지울 수 있으면 그 이름, 아니면 `brv.old.<unix초>`.
///
/// 2026-09-14 실사고: 0.7.2 갱신 때 `brv.old`(직전 갱신이 비켜 둔 0.7.0)를 앱 안에 남아 있던 옛 중계기가 아직 쥐고 있어
/// 이름 바꾸기가 실패했고, 서비스는 0.7.1로 남은 채 새 0.7.2 중계기가 "버전이 다르다"며 거부됐다 — 갱신이 절반만 닿은
/// 상태(수칙 9 위반). 비켜 둘 이름이 막혀 있다고 갱신을 포기하지 않는다. 고유 이름의 잔재는 [`sweep_parked_binaries`]가
/// 다음 기동·재기동 때 치운다.
fn park_aside(registered: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let parked = registered.with_extension("old");
    let _ = std::fs::remove_file(&parked); // 지난 갱신의 잔재 (아직 실행 중이면 실패 — 아래서 다른 이름을 쓴다)
    match std::fs::rename(registered, &parked) {
        Ok(()) => Ok(parked),
        Err(first) => {
            let unix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let unique = registered.with_extension(format!("old.{unix}"));
            match std::fs::rename(registered, &unique) {
                Ok(()) => Ok(unique),
                Err(second) => Err(std::io::Error::new(
                    second.kind(),
                    format!(
                        "{} ({first}); {} ({second})",
                        parked.display(),
                        unique.display()
                    ),
                )),
            }
        }
    }
}

/// 같은 파일인가 — 경로 표기 차이(대소문자·구분자·`.`)를 정규화해 비교한다.
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    let norm = |p: &std::path::Path| {
        std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// `<exe> --version` 한 줄. 못 읽으면 None(판단 불가 → 건드리지 않는다).
fn probe_version(exe: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new(exe)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(str::to_owned)
}

/// Some(Some(path)) = 등록됨·전용 프로필, Some(None) = 등록됨·기본 경로, None = 미등록.
#[cfg(target_os = "linux")]
fn registration() -> Option<Option<String>> {
    let unit = dirs::config_dir()?
        .join("systemd")
        .join("user")
        .join(format!("{SERVICE_NAME}.service"));
    let text = std::fs::read_to_string(unit).ok()?;
    Some(config_from_unit(&text))
}

#[cfg(target_os = "macos")]
fn registration() -> Option<Option<String>> {
    // 옛 방식 plist가 있으면 그것이 진실, 없으면 SMAppService 등록의 표지 파일 (macos_bundle.rs).
    // 이 함수는 모든 CLI 명령의 설정 경로 해석에 쓰인다 — 프로세스를 띄우지 않고 파일만 본다
    if let Ok(text) = std::fs::read_to_string(legacy_plist()?) {
        return Some(config_from_plist(&text));
    }
    crate::macos_bundle::read_marker().map(|env| env.config)
}

/// 옛 방식 등록 파일 — `~/Library/LaunchAgents/dev.brevduva.brv-daemon.plist`.
#[cfg(target_os = "macos")]
fn legacy_plist() -> Option<std::path::PathBuf> {
    Some(
        dirs::home_dir()?
            .join("Library/LaunchAgents")
            .join(format!("{LAUNCHD_LABEL}.plist")),
    )
}

#[cfg(windows)]
fn registration() -> Option<Option<String>> {
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).ok()?;
    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::QUERY_CONFIG)
        .ok()?;
    let config = service.query_config().ok()?;
    // executable_path는 SCM의 BinaryPathName — 실행 파일과 인자가 한 줄로 들어 있다
    Some(config_from_command_line(
        &config.executable_path.to_string_lossy(),
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn registration() -> Option<Option<String>> {
    None
}

#[cfg(target_os = "linux")]
fn registration_exe() -> Option<std::path::PathBuf> {
    let unit = dirs::config_dir()?
        .join("systemd")
        .join("user")
        .join(format!("{SERVICE_NAME}.service"));
    exe_from_unit(&std::fs::read_to_string(unit).ok()?).map(std::path::PathBuf::from)
}

/// 옛 방식 plist의 실행 파일만 돌려준다. SMAppService 등록(앱 묶음)은 None — [`align_binary`]가
/// **서명으로 봉인된 묶음 안에 파일을 복사해 넣으면 서명이 깨져** macOS가 실행을 막는다. 묶음의
/// 갱신은 설치기가 묶음째 교체하는 것이 유일한 길이다.
#[cfg(target_os = "macos")]
fn registration_exe() -> Option<std::path::PathBuf> {
    exe_from_plist(&std::fs::read_to_string(legacy_plist()?).ok()?).map(std::path::PathBuf::from)
}

#[cfg(windows)]
fn registration_exe() -> Option<std::path::PathBuf> {
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).ok()?;
    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::QUERY_CONFIG)
        .ok()?;
    let config = service.query_config().ok()?;
    // BinaryPathName = 실행 파일 + 인자 한 줄 — 첫 토큰이 실행 파일
    tokenize(&config.executable_path.to_string_lossy())
        .into_iter()
        .next()
        .map(std::path::PathBuf::from)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn registration_exe() -> Option<std::path::PathBuf> {
    None
}

/// systemd 유닛의 `ExecStart=<exe> daemon` 첫 토큰.
#[cfg(any(target_os = "linux", test))]
fn exe_from_unit(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("ExecStart="))
        .and_then(|v| tokenize(v).into_iter().next())
}

/// launchd plist의 `ProgramArguments` 첫 `<string>`.
#[cfg(any(target_os = "macos", test))]
fn exe_from_plist(text: &str) -> Option<String> {
    let after = text.split("<key>ProgramArguments</key>").nth(1)?;
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")? + start;
    Some(after[start..end].to_owned())
}

/// systemd 유닛에서 `Environment=BREVDUVA_CONFIG=…` 값.
#[cfg(any(target_os = "linux", test))]
fn config_from_unit(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        l.trim()
            .strip_prefix("Environment=BREVDUVA_CONFIG=")
            .map(|v| v.trim().trim_matches('"').to_owned())
    })
}

/// launchd plist에서 `<key>BREVDUVA_CONFIG</key><string>…</string>` 값.
#[cfg(any(target_os = "macos", test))]
fn config_from_plist(text: &str) -> Option<String> {
    let after = text.split("<key>BREVDUVA_CONFIG</key>").nth(1)?;
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")? + start;
    Some(after[start..end].to_owned())
}

/// 명령줄 한 줄 → 토큰 — 따옴표로 묶인 토큰은 하나로 본다 (공백 있는 경로).
#[cfg(any(windows, target_os = "linux", test))]
fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// SCM BinaryPathName에서 `--config` 다음 토큰.
#[cfg(any(windows, test))]
fn config_from_command_line(line: &str) -> Option<String> {
    let tokens = tokenize(line);
    tokens
        .iter()
        .position(|t| t == "--config")
        .and_then(|i| tokens.get(i + 1).cloned())
}

/// 서비스 정의에 심을 설정 경로 검증 — 서비스는 다른 cwd에서 뜨므로 절대 경로만 허용.
fn require_absolute(config: Option<&str>) -> anyhow::Result<Option<&str>> {
    if let Some(c) = config {
        anyhow::ensure!(
            std::path::Path::new(c).is_absolute(),
            "--config must be an absolute path (services start in a different working directory): {c}"
        );
    }
    Ok(config)
}

// ---------------------------------------------------------------- Linux: systemd 사용자 유닛

#[cfg(target_os = "linux")]
pub fn install(config: Option<&str>) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let config = require_absolute(config)?;
    let exe = std::env::current_exe()?;
    // 설치자 셸의 PATH를 유닛에 굽는다 (2026-08-29 실사고): systemd 기본 PATH에는
    // ~/.local/bin 등이 없어 wake 명령("claude")을 못 찾는다 — 데몬은 메시지를 소비하고도
    // 깨우기만 조용히 실패했다. %는 systemd 지정자라 이스케이프.
    let path_line = std::env::var("PATH")
        .map(|p| format!("Environment=\"PATH={}\"\n", p.replace('%', "%%")))
        .unwrap_or_default();
    let env_line = format!(
        "{path_line}{}",
        config
            .map(|c| format!("Environment=BREVDUVA_CONFIG={c}\n"))
            .unwrap_or_default()
    );
    let unit = format!(
        "# `brv daemon install`이 생성 — 수동 수정은 재실행 시 덮어써진다\n\
         [Unit]\n\
         Description=Brevduva receiver daemon (brv)\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         {env_line}ExecStart={exe} daemon\n\
         Restart=always\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exe = exe.display(),
    );
    let dir = dirs::config_dir()
        .context("cannot resolve config dir")?
        .join("systemd")
        .join("user");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{SERVICE_NAME}.service")), unit)?;
    run_cmd("systemctl", &["--user", "daemon-reload"])?;
    run_cmd("systemctl", &["--user", "enable", "--now", SERVICE_NAME])?;
    // linger: 사용자 매니저가 부팅 시(로그인 없이) 뜨게 — 없으면 SSH/로그인 후에만 기동
    if run_cmd("loginctl", &["enable-linger"]).is_err() {
        let user = std::env::var("USER").unwrap_or_default();
        println!(
            "warning: enabling linger failed — for start-on-boot run manually: sudo loginctl enable-linger {user}"
        );
    }
    println!(
        "registered: systemd user unit {SERVICE_NAME} (logs: journalctl --user -u {SERVICE_NAME} -f)"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn uninstall() -> anyhow::Result<()> {
    use anyhow::Context as _;
    // 정지 실패(이미 없음 등)는 무시 — 목표는 제거의 멱등성
    let _ = run_cmd("systemctl", &["--user", "disable", "--now", SERVICE_NAME]);
    let unit = dirs::config_dir()
        .context("cannot resolve config dir")?
        .join("systemd")
        .join("user")
        .join(format!("{SERVICE_NAME}.service"));
    if unit.exists() {
        std::fs::remove_file(&unit)?;
    }
    let _ = run_cmd("systemctl", &["--user", "daemon-reload"]);
    println!("unregistered: {SERVICE_NAME}");
    Ok(())
}

/// 등록된 서비스 재기동 (2026-09-02, 맥북 실사고: 설정·토큰을 바꿔도 돌던 데몬은 모른다).
/// Ok(false) = 서비스 미등록 (호출자가 "직접 재시작하라" 안내). 설정 변경 명령들이 호출한다.
#[cfg(target_os = "linux")]
pub fn restart() -> anyhow::Result<bool> {
    use anyhow::Context as _;
    let unit = dirs::config_dir()
        .context("cannot resolve config dir")?
        .join("systemd")
        .join("user")
        .join(format!("{SERVICE_NAME}.service"));
    if !unit.exists() {
        return Ok(false);
    }
    run_cmd("systemctl", &["--user", "restart", SERVICE_NAME])?;
    Ok(true)
}

// ---------------------------------------------------------------- macOS: LaunchAgent

#[cfg(target_os = "macos")]
use crate::macos_bundle::LAUNCHD_LABEL;

#[cfg(target_os = "macos")]
pub fn install(config: Option<&str>) -> anyhow::Result<()> {
    let config = require_absolute(config)?;
    let Some(helper) = crate::macos_bundle::bundled_helper() else {
        return install_legacy(config);
    };
    // 앱 묶음 설치: SMAppService로 등록한다. 같은 이름의 옛 방식 등록이 남아 있으면 먼저 내린다 —
    // 둘이 동시에 있으면 launchd가 어느 쪽을 띄울지 알 수 없고 시스템 설정에는 개발자 이름이 남는다
    let legacy = remove_legacy_registration();
    // 이미 새 방식으로 돌고 있었는가 — 그러면 등록은 아무것도 다시 띄우지 않아 방금 적은 표지(PATH·프로필)가
    // 다음 재기동까지 적용되지 않는다 (2026-09-22 맥북 실측). 옛 방식 install은 내렸다 올려서 바로 적용됐다
    let already_running = legacy.is_none()
        && crate::macos_bundle::status(&helper) == crate::macos_bundle::HelperStatus::Enabled;
    if let Err(e) = crate::macos_bundle::register(
        &helper,
        &crate::macos_bundle::ServiceEnv {
            path: std::env::var("PATH").ok(),
            config: config.map(str::to_owned),
        },
    ) {
        if legacy.is_some() {
            restore_legacy_registration(legacy);
            return Err(e);
        }
        // 되살릴 옛 등록도 없다 — 서비스가 아예 없는 머신으로 두지 않는다 (P1: 리시버는 OS 서비스로 상주).
        // 옛 방식으로라도 등록하고, 표시가 개발자 이름으로 나온다는 것과 이유를 알린다
        eprintln!(
            "warning: {e:#} — registering the receiver the older way instead (System Settings will list it under the developer's name)"
        );
        return install_legacy(config);
    }
    if already_running {
        kickstart()?;
    }
    println!(
        "registered: Brevduva background service {LAUNCHD_LABEL} (starts at login, logs: ~/Library/Logs/brv-daemon.log)"
    );
    Ok(())
}

/// 옛 방식 등록을 내리고 plist를 지운다 (없으면 아무 일도 없다). 지운 plist의 내용을 돌려준다 —
/// 새 방식 등록이 실패하면 [`restore_legacy_registration`]이 되살린다.
#[cfg(target_os = "macos")]
fn remove_legacy_registration() -> Option<String> {
    let plist = legacy_plist().filter(|p| p.exists())?;
    let text = std::fs::read_to_string(&plist).ok();
    if let Ok(uid) = launchd_uid() {
        let _ = run_cmd(
            "launchctl",
            &["bootout", &format!("gui/{uid}/{LAUNCHD_LABEL}")],
        );
    }
    if let Err(e) = std::fs::remove_file(&plist) {
        eprintln!("warning: could not remove the old registration {plist:?}: {e}");
    }
    text
}

/// 새 방식 등록이 실패했을 때 옛 방식 등록을 되살린다 — 이전 도중의 실패가 "서비스가 아예 없는 머신"을
/// 남기면 안 된다 (메시지를 받을 곳이 사라진다). 되살리기까지 실패하면 사용자가 알아야 한다.
#[cfg(target_os = "macos")]
fn restore_legacy_registration(text: Option<String>) {
    let (Some(text), Some(plist)) = (text, legacy_plist()) else {
        return;
    };
    let restored = std::fs::write(&plist, text)
        .map_err(anyhow::Error::from)
        .and_then(|()| launchd_uid())
        .and_then(|uid| {
            run_cmd(
                "launchctl",
                &["bootstrap", &format!("gui/{uid}"), &plist.to_string_lossy()],
            )
        });
    match restored {
        Ok(()) => eprintln!("kept the previous background service registration"),
        Err(e) => eprintln!(
            "warning: the previous registration could not be restored ({e:#}) — run `brv daemon install`"
        ),
    }
}

#[cfg(target_os = "macos")]
fn launchd_uid() -> anyhow::Result<String> {
    Ok(
        String::from_utf8(std::process::Command::new("id").arg("-u").output()?.stdout)?
            .trim()
            .to_owned(),
    )
}

/// 옛 방식: `~/Library/LaunchAgents`에 plist를 쓴다 — 단독 실행 파일 설치와 macOS 13 미만.
#[cfg(target_os = "macos")]
fn install_legacy(config: Option<&str>) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let exe = std::env::current_exe()?;
    let home = dirs::home_dir().context("cannot resolve home dir")?;
    let log = home.join("Library/Logs/brv-daemon.log");
    // 설치자 셸의 PATH를 굽는다 (2026-08-29 실사고 — systemd와 동일: launchd 기본 PATH에는
    // 사용자 설치 경로가 없어 wake 명령을 못 찾는다). XML 이스케이프는 &·< 만 실질 위험.
    let path_entry = std::env::var("PATH")
        .map(|p| {
            let p = p.replace('&', "&amp;").replace('<', "&lt;");
            format!("    <key>PATH</key><string>{p}</string>\n")
        })
        .unwrap_or_default();
    let config_entry = config
        .map(|c| format!("    <key>BREVDUVA_CONFIG</key><string>{c}</string>\n"))
        .unwrap_or_default();
    let env_block = format!(
        "  <key>EnvironmentVariables</key>\n  <dict>\n{path_entry}{config_entry}  </dict>\n"
    );
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>daemon</string>
  </array>
{env_block}  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
        exe = exe.display(),
        log = log.display(),
    );
    let dir = home.join("Library/LaunchAgents");
    std::fs::create_dir_all(&dir)?;
    let plist_path = dir.join(format!("{LAUNCHD_LABEL}.plist"));
    std::fs::write(&plist_path, plist)?;
    let uid = launchd_uid()?;
    // 이미 로드돼 있으면 bootstrap이 거부한다 — 먼저 내리고(실패 무시) 다시 올린다
    let _ = run_cmd(
        "launchctl",
        &["bootout", &format!("gui/{uid}/{LAUNCHD_LABEL}")],
    );
    run_cmd(
        "launchctl",
        &[
            "bootstrap",
            &format!("gui/{uid}"),
            plist_path.to_str().context("plist path utf-8")?,
        ],
    )?;
    println!(
        "registered: LaunchAgent {LAUNCHD_LABEL} (starts at login, logs: {})",
        log.display()
    );
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn uninstall() -> anyhow::Result<()> {
    // 두 등록 길을 다 내린다 — 어느 쪽으로 등록됐든 남는 것이 없어야 한다
    if let Some(helper) = crate::macos_bundle::bundled_helper() {
        crate::macos_bundle::unregister(&helper);
    }
    let _ = remove_legacy_registration();
    println!("unregistered: {LAUNCHD_LABEL}");
    Ok(())
}

/// 등록된 LaunchAgent 재기동 — `kickstart -k`는 돌고 있으면 죽이고 다시 띄운다 (linux restart 참조).
///
/// 설치기는 갱신 뒤 이 명령 하나만 부른다. 그래서 **단독 실행 파일 → 앱 묶음 갱신의 등록 이전도
/// 여기서 끝낸다** (2026-09-22, 수칙 8·9): 묶음 안에서 실행됐는데 옛 방식 plist가 남아 있으면 그
/// plist의 PATH·프로필을 이어받아 SMAppService로 다시 등록하고 plist를 지운다.
#[cfg(target_os = "macos")]
pub fn restart() -> anyhow::Result<bool> {
    use crate::macos_bundle::{self as bundle, HelperStatus};
    let legacy = legacy_plist().filter(|p| p.exists());
    let Some(helper) = bundle::bundled_helper() else {
        // 단독 실행 파일 (또는 macOS 13 미만): 옛 방식 그대로
        if legacy.is_none() {
            return Ok(false);
        }
        kickstart()?;
        return Ok(true);
    };
    if let Some(plist) = legacy {
        let text = std::fs::read_to_string(&plist).unwrap_or_default();
        let env = bundle::ServiceEnv {
            path: bundle::env_from_plist(&text, "PATH"),
            config: config_from_plist(&text),
        };
        let removed = remove_legacy_registration();
        // 등록하면 launchd가 바로 띄운다 — kickstart는 필요 없다
        if let Err(e) = bundle::register(&helper, &env) {
            restore_legacy_registration(removed);
            return Err(e);
        }
        println!("moved the background service registration into Brevduva.app");
        return Ok(true);
    }
    if bundle::read_marker().is_none() {
        return Ok(false);
    }
    // 묶음이 통째로 교체된 뒤 macOS가 등록을 놓았을 수 있다 — 상태를 보고 필요하면 다시 등록한다
    if bundle::status(&helper) != HelperStatus::Enabled {
        let env = bundle::read_marker().unwrap_or_default();
        bundle::register(&helper, &env)?;
        return Ok(true);
    }
    kickstart()?;
    Ok(true)
}

#[cfg(target_os = "macos")]
fn kickstart() -> anyhow::Result<()> {
    let uid = launchd_uid()?;
    run_cmd(
        "launchctl",
        &["kickstart", "-k", &format!("gui/{uid}/{LAUNCHD_LABEL}")],
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_cmd(program: &str, args: &[&str]) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let status = std::process::Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to run {program} — is it installed?"))?;
    anyhow::ensure!(status.success(), "{program} {args:?} failed ({status})");
    Ok(())
}

// ---------------------------------------------------------------- Windows: SCM 서비스 (LocalSystem)

/// 서비스 실행 인자(`--wake-user`)로 받은 깨울 사용자 — svc_main은 함수 포인터라 늦게 공급.
#[cfg(windows)]
static WAKE_USER: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

#[cfg(windows)]
pub fn install(config: Option<&str>) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::ffi::OsString;
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    // 설정 경로는 항상 절대 경로로 굽는다 — 시스템 계정의 기본 경로는 사용자 것과 다르다
    let config_path = match require_absolute(config)? {
        Some(c) => std::path::PathBuf::from(c),
        None => crate::config::config_path()?,
    };
    crate::config::set_path_override(config_path.clone());
    let exe = std::env::current_exe()?;
    let user = std::env::var("USERNAME").context("USERNAME env")?;

    // UAC 자기 승격 (2026-09-04, 온보딩 재설계 2): 관리자 터미널을 따로 열게 하지 않는다 —
    // 같은 명령을 승격해 다시 실행하고 기다린다(클릭 1회, 조용한 승격은 없다). 승격된 프로세스는
    // 같은 사용자(USERNAME 동일)라 소유자 판단이 유지되고, 출력은 파일로 받아 여기서 보여준다
    if !crate::winspawn::is_elevated() {
        let log = std::env::temp_dir().join("brv-daemon-install.log");
        let _ = std::fs::remove_file(&log);
        let args = vec![
            "daemon".to_owned(),
            "install".to_owned(),
            "--config".to_owned(),
            config_path.to_string_lossy().into_owned(),
        ];
        println!(
            "registering the Windows service needs administrator rights — approve the prompt (once)"
        );
        let code = crate::winspawn::run_elevated(&exe, &args, &log)?;
        if let Ok(out) = std::fs::read_to_string(&log) {
            print!("{out}");
        }
        anyhow::ensure!(code == 0, "elevated install exited with code {code}");
        return Ok(());
    }

    // 토큰을 파일로 — 시스템 계정은 사용자의 자격 증명 저장소를 못 본다. 윈도우의 load_token은
    // 파일 우선 + 저장소→파일 자가 이전이라, 여기서 한 번 돌려 서비스가 읽을 파일을 보장한다.
    // 평문 파일이 되므로 디렉터리 권한을 먼저 좁힌다 (2026-09-03 — config::secure_config_dir)
    crate::config::secure_config_dir();
    let cfg = crate::config::load()?;
    crate::config::load_tokens(&cfg)
        .context("every binding's token must be readable before registering the service")?;

    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)
            .context("failed to open the service manager — run this once from an administrator terminal (Run as administrator)")?;

    let launch_arguments = vec![
        OsString::from("daemon"),
        OsString::from("service-run"),
        OsString::from("--config"),
        config_path.clone().into_os_string(),
        OsString::from("--wake-user"),
        OsString::from(&user),
    ];
    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: "Brevduva Receiver Daemon".into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments,
        dependencies: vec![],
        account_name: None, // LocalSystem — 암호 없음
        account_password: None,
    };
    let service = manager
        .create_service(&info, ServiceAccess::START)
        .context("failed to create the service — if already registered, `brv daemon uninstall` and retry")?;
    // 소유자가 승격 없이 재기동할 수 있게 — 설정 변경 명령의 자동 재기동이 이 권한에 기댄다
    match grant_service_control(&user) {
        Ok(()) => println!(
            "service control granted to {user} — `brv daemon restart` works without elevation"
        ),
        Err(e) => println!(
            "warning: could not grant service control to {user} — `brv daemon restart` will need an administrator terminal: {e}"
        ),
    }
    // lucadm 실측(2026-09-03): 같은 설정을 쓰는 데몬 둘은 자리 다툼·메시지 가로채기를 일으킨다
    if scheduled_task_exists() {
        println!(
            "warning: a scheduled task named {SERVICE_NAME} also runs a daemon — two daemons compete for the agent slot. Disable it: schtasks /Change /TN {SERVICE_NAME} /DISABLE"
        );
    }
    service
        .start::<&std::ffi::OsStr>(&[])
        .context("failed to start the service — see daemon-service.log in the config directory")?;
    println!(
        "registered and started: service {SERVICE_NAME} (LocalSystem, starts at boot) — wakes run in {user}'s logged-on session: locked is fine, logged out = waits. Logs: daemon-service.log in the config directory"
    );
    Ok(())
}

/// 서비스 시작·정지·상태 조회 권한을 설치한 사용자에게 — SCM 기본 DACL은 관리자만 제어할
/// 수 있어 `brv daemon restart`(설정 변경 후 자동 호출)가 일반 프롬프트에서 거부된다.
/// SDDL 편집은 `sc.exe sdshow/sdset`로 — 보안 설명자 API의 FFI를 늘리지 않는다.
#[cfg(windows)]
fn grant_service_control(user: &str) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let sid = crate::config::user_sid().with_context(|| format!("SID of {user}"))?;
    let out = std::process::Command::new("sc.exe")
        .args(["sdshow", SERVICE_NAME])
        .output()
        .context("sc.exe sdshow")?;
    anyhow::ensure!(
        out.status.success(),
        "sc sdshow failed: {}",
        String::from_utf8_lossy(&out.stdout).trim()
    );
    let sddl = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    // CC LC SW RP WP DT LO CR RC = 구성 조회·상태 조회·열거·시작·정지·일시정지·조사·사용자
    // 제어·읽기 — services.msc의 "시작/중지" 권한 묶음 (삭제·구성 변경은 여전히 관리자)
    let ace = format!("(A;;CCLCSWRPWPDTLOCRRC;;;{sid})");
    if sddl.contains(&ace) {
        return Ok(());
    }
    // DACL(D:) 끝, SACL(S:) 앞에 끼운다
    let new = match sddl.find("S:") {
        Some(i) => format!("{}{ace}{}", &sddl[..i], &sddl[i..]),
        None => format!("{sddl}{ace}"),
    };
    let status = std::process::Command::new("sc.exe")
        .args(["sdset", SERVICE_NAME, &new])
        .status()
        .context("sc.exe sdset")?;
    anyhow::ensure!(status.success(), "sc sdset failed ({status})");
    Ok(())
}

/// 같은 이름의 작업 스케줄러 작업이 **활성** 상태인가 — 비활성(Disabled)은 경고하지 않는다.
#[cfg(windows)]
fn scheduled_task_exists() -> bool {
    std::process::Command::new("schtasks")
        .args(["/Query", "/TN", SERVICE_NAME, "/FO", "CSV", "/NH"])
        .output()
        .map(|o| {
            // "\brv-daemon","N/A","Disabled" — 마지막 열이 상태
            o.status.success() && !String::from_utf8_lossy(&o.stdout).contains("\"Disabled\"")
        })
        .unwrap_or(false)
}

#[cfg(windows)]
pub fn uninstall() -> anyhow::Result<()> {
    use anyhow::Context as _;
    use windows_service::service::{ServiceAccess, ServiceState};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("failed to open the service manager")?;
    let service = manager
        .open_service(
            SERVICE_NAME,
            ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
        )
        .context("the service is not registered (or administrator rights are required)")?;
    if service
        .query_status()
        .map(|s| s.current_state != ServiceState::Stopped)
        .unwrap_or(false)
    {
        let _ = service.stop();
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if service
                .query_status()
                .map(|s| s.current_state == ServiceState::Stopped)
                .unwrap_or(true)
            {
                break;
            }
        }
    }
    service.delete().context("failed to delete the service")?;
    println!("unregistered: {SERVICE_NAME}");
    Ok(())
}

/// 등록된 SCM 서비스 재기동 (linux restart 참조). 미등록이면 Ok(false) — 작업 스케줄러 등
/// 다른 상주 방식은 여기서 모른다 (호출자가 직접 재시작 안내).
#[cfg(windows)]
pub fn restart() -> anyhow::Result<bool> {
    use anyhow::Context as _;
    use windows_service::service::{ServiceAccess, ServiceState};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("failed to open the service manager")?;
    let Ok(service) = manager.open_service(
        SERVICE_NAME,
        ServiceAccess::STOP | ServiceAccess::START | ServiceAccess::QUERY_STATUS,
    ) else {
        return Ok(false);
    };
    if service
        .query_status()
        .map(|s| s.current_state != ServiceState::Stopped)
        .unwrap_or(false)
    {
        let _ = service.stop();
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if service
                .query_status()
                .map(|s| s.current_state == ServiceState::Stopped)
                .unwrap_or(true)
            {
                break;
            }
        }
    }
    service
        .start::<&std::ffi::OsStr>(&[])
        .context("failed to start the service (administrator rights may be required)")?;
    Ok(true)
}

/// SCM이 서비스 프로세스로 이 바이너리를 띄울 때의 진입점 (`brv daemon service-run`).
/// 콘솔이 없으므로 로그는 main에서 파일로 초기화돼 있다. 설정 경로 override도 main에서.
/// `wake_user` = 설치자 (깨우기를 그 사용자 세션에 — 없으면 활성 세션 아무거나).
#[cfg(windows)]
pub fn service_run(wake_user: Option<String>) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let _ = WAKE_USER.set(wake_user);
    windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("SCM dispatcher failed — service-run is SCM-only (never run directly)")?;
    Ok(())
}

#[cfg(windows)]
windows_service::define_windows_service!(ffi_service_main, svc_main);

#[cfg(windows)]
fn svc_main(_launch_args: Vec<std::ffi::OsString>) {
    if let Err(e) = run_service() {
        tracing::error!(error = %e, "service run failed");
    }
}

#[cfg(windows)]
fn run_service() -> anyhow::Result<()> {
    use std::sync::Arc;
    use std::time::Duration;
    use windows_service::service::{ServiceControl, ServiceControlAccept, ServiceState};
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    // 핸들은 register 이후에야 생기므로 OnceLock으로 핸들러 클로저에 늦게 공급
    let handle_cell = Arc::new(std::sync::OnceLock::new());
    let handle_for_events = Arc::clone(&handle_cell);
    let status_handle = service_control_handler::register(SERVICE_NAME, move |control| {
        match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                // wake가 진행 중이면 완료 후 종료한다(데몬 설계) — SCM에 넉넉한 유예를 알림
                if let Some(h) = handle_for_events.get() {
                    let _ = set_status(
                        h,
                        ServiceState::StopPending,
                        ServiceControlAccept::empty(),
                        Duration::from_secs(660),
                    );
                }
                let _ = stop_tx.send(true);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    })?;
    let _ = handle_cell.set(status_handle);
    set_status(
        &status_handle,
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        Duration::ZERO,
    )?;

    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(async {
        let cfg = crate::config::load()?;
        // 전 바인딩 토큰 일괄 로드 (페이즈 27) — 하나라도 없으면 기동 거부가 정직하다
        let tokens = crate::config::load_tokens(&cfg)?;
        let reload_cfg = cfg.clone();
        crate::daemon::run_with_options(
            cfg,
            tokens,
            crate::daemon::DaemonOptions {
                shutdown: Some(stop_rx),
                token_reload: Some(std::sync::Arc::new(move |b: &crate::config::Binding| {
                    crate::config::load_token(&reload_cfg, b).ok()
                })),
                preflight: true,
                // 시스템 계정은 claude 로그인·프로젝트를 못 연다 — 깨우기는 사용자 세션에서
                wake_spawn: crate::daemon::WakeSpawn::UserSession {
                    user: WAKE_USER.get().cloned().flatten(),
                },
                ..Default::default()
            },
        )
        .await
    });
    // 실패는 **Stopped 보고 전에** 기록한다 (2026-09-04 실측): Stopped를 알리면 SCM이 프로세스를
    // 곧바로 정리할 수 있어, 그 뒤의 로그 한 줄은 파일에 닿지 못한다 — 실제로 기동 실패가
    // `sc query` 종료 코드 0 + 로그 0바이트로 완전히 침묵했다(설정 오류인데 원인을 알 길이 없음).
    // 종료 코드도 실패면 1로 — "깨끗이 멈춤"과 "떠보지도 못함"은 다른 사실이다.
    let failed = match &result {
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "daemon could not run — service stops");
            true
        }
        Ok(()) => false,
    };
    set_status_code(
        &status_handle,
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        Duration::ZERO,
        u32::from(failed),
    )?;
    result
}

#[cfg(windows)]
fn set_status(
    handle: &windows_service::service_control_handler::ServiceStatusHandle,
    state: windows_service::service::ServiceState,
    accept: windows_service::service::ServiceControlAccept,
    wait_hint: std::time::Duration,
) -> windows_service::Result<()> {
    set_status_code(handle, state, accept, wait_hint, 0)
}

/// 종료 코드까지 지정하는 상태 보고 — 기동 실패를 `sc query`가 드러내게 (2026-09-04).
#[cfg(windows)]
fn set_status_code(
    handle: &windows_service::service_control_handler::ServiceStatusHandle,
    state: windows_service::service::ServiceState,
    accept: windows_service::service::ServiceControlAccept,
    wait_hint: std::time::Duration,
    exit_code: u32,
) -> windows_service::Result<()> {
    use windows_service::service::{ServiceExitCode, ServiceStatus, ServiceType};
    handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: ServiceExitCode::ServiceSpecific(exit_code),
        checkpoint: 0,
        wait_hint,
        process_id: None,
    })
}

// ---------------------------------------------------------------- 그 외 OS

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn install(_config: Option<&str>) -> anyhow::Result<()> {
    anyhow::bail!(
        "service registration is not supported on this OS — run `brv daemon` directly as a resident process"
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn uninstall() -> anyhow::Result<()> {
    anyhow::bail!("service registration is not supported on this OS")
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn restart() -> anyhow::Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod registration_tests {
    use super::*;

    /// 2026-09-14 (P8, 수칙 9): 비켜 둘 이름 `brv.old`가 이미 있어도(직전 갱신의 잔재) 새 파일을 비켜 두는 데 실패하지
    /// 않는다 — 지울 수 있으면 그 이름을 다시 쓰고, 어느 이름이든 나중에 청소 대상이다.
    #[test]
    fn a_service_binary_is_parked_even_when_the_previous_parking_name_is_taken() {
        let dir = std::env::temp_dir().join(format!("brv-park-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let registered = dir.join("brv.exe");
        std::fs::write(&registered, b"new").unwrap();
        std::fs::write(dir.join("brv.old"), b"previous").unwrap();
        let parked = park_aside(&registered).unwrap();
        assert!(
            !registered.exists(),
            "the registered path is free for the copy"
        );
        assert_eq!(std::fs::read(&parked).unwrap(), b"new");
        let name = parked.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name == "brv.old" || name.starts_with("brv.old."), "{name}");
        let swept = sweep_parked_binaries(&registered);
        assert!(
            swept.contains(&parked),
            "whichever name was used is an update leftover"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 2026-09-13 (P8): 러너 등록은 brv 버전이 바뀐 뒤 한 번만 다시 쓴다 — 표시 파일이 없거나 다른 버전이면
    /// 낡은 것이고, 지금 버전으로 표시하면 다음 재기동에서는 건드리지 않는다.
    #[test]
    fn runner_registrations_are_rewritten_once_per_brv_version() {
        let dir = std::env::temp_dir().join(format!("brv-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        assert!(
            registrations_stale(&config),
            "no stamp = never registered by this version"
        );
        std::fs::write(
            registration_stamp_path(&config),
            "0.6.39
",
        )
        .unwrap();
        assert!(
            registrations_stale(&config),
            "a stamp from another version = updated since"
        );
        stamp_registrations(&config).unwrap();
        assert!(!registrations_stale(&config));
        assert_eq!(
            std::fs::read_to_string(dir.join("mcp-registered.version"))
                .unwrap()
                .trim(),
            env!("CARGO_PKG_VERSION")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 2026-09-04: 세 OS의 등록 파일/SCM 문자열에서 프로필 경로를 읽는다 — 공백·따옴표 포함.
    #[test]
    fn registered_profile_is_read_from_each_registration_form() {
        assert_eq!(
            config_from_unit(
                "[Service]\nEnvironment=\"PATH=/a b\"\nEnvironment=BREVDUVA_CONFIG=/home/u/.config/brevduva/config.toml\nExecStart=/x daemon\n"
            ),
            Some("/home/u/.config/brevduva/config.toml".to_owned())
        );
        assert_eq!(config_from_unit("[Service]\nExecStart=/x daemon\n"), None);
        assert_eq!(
            config_from_plist(
                "<dict>\n<key>PATH</key><string>/usr/bin</string>\n<key>BREVDUVA_CONFIG</key><string>/Users/u/proj/config.toml</string>\n</dict>"
            ),
            Some("/Users/u/proj/config.toml".to_owned())
        );
        assert_eq!(config_from_plist("<dict></dict>"), None);
        assert_eq!(
            config_from_command_line(
                r#""C:\brevduva\brv.exe" daemon service-run --config "C:\my profile\config.toml" --wake-user Jaeyoung"#
            ),
            Some(r"C:\my profile\config.toml".to_owned())
        );
        assert_eq!(
            config_from_command_line(
                r"C:\brevduva\brv.exe daemon service-run --config C:\brevduva\config.toml --wake-user Jaeyoung"
            ),
            Some(r"C:\brevduva\config.toml".to_owned())
        );
        assert_eq!(
            config_from_command_line(r"C:\brevduva\brv.exe daemon"),
            None
        );
    }

    /// 2026-09-04: 갱신이 서비스에 닿으려면 **서비스가 실행하는 파일**을 알아야 한다 —
    /// 설치기는 CLI 경로만 바꾸므로, 등록 경로가 다르면 그 파일을 교체해야 한다 (align_binary).
    #[test]
    fn registered_executable_is_read_from_each_registration_form() {
        assert_eq!(
            exe_from_unit(
                "[Service]\nEnvironment=BREVDUVA_CONFIG=/c\nExecStart=/home/u/.local/bin/brv daemon\n"
            ),
            Some("/home/u/.local/bin/brv".to_owned())
        );
        assert_eq!(exe_from_unit("[Service]\nRestart=always\n"), None);
        assert_eq!(
            exe_from_plist(
                "<dict>\n<key>ProgramArguments</key>\n<array>\n<string>/Users/u/.local/bin/brv</string>\n<string>daemon</string>\n</array>\n</dict>"
            ),
            Some("/Users/u/.local/bin/brv".to_owned())
        );
        assert_eq!(exe_from_plist("<dict></dict>"), None);
        // 공백 있는 경로는 따옴표 안이 한 토큰
        assert_eq!(
            tokenize(r#""C:\Program Files\brv.exe" daemon service-run"#)
                .first()
                .map(String::as_str),
            Some(r"C:\Program Files\brv.exe")
        );
    }
}
