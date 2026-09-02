// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! OS 서비스 등록·해제 — `brv daemon install` / `uninstall` (페이즈 7).
//!
//! 공통 설계: 서비스는 **현재 사용자 컨텍스트**로 돈다 — 토큰 키체인과 wake용 CLI
//! 로그인(claude 등)이 사용자 프로필 소속이라 시스템 계정(LocalSystem/LaunchDaemon)은
//! 동작하지 않기 때문. 머신당 서비스 1개(이름 고정), 프로필 선택은 `--config`로.
//!
//! - Linux: systemd 사용자 유닛 + `loginctl enable-linger`(부팅 시 로그인 없이 기동)
//! - macOS: LaunchAgent (로그인 시 기동 + KeepAlive)
//! - Windows: SCM 진짜 서비스 (부팅 시 기동). 설치 시 계정 암호 1회 입력(SCM 저장) +
//!   `SeServiceLogonRight` LSA 부여 (services.msc GUI와 달리 API 등록은 자동 부여가 없다)

pub const SERVICE_NAME: &str = "brv-daemon";

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
const LAUNCHD_LABEL: &str = "dev.brevduva.brv-daemon";

#[cfg(target_os = "macos")]
pub fn install(config: Option<&str>) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let config = require_absolute(config)?;
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
    let uid = String::from_utf8(std::process::Command::new("id").arg("-u").output()?.stdout)?
        .trim()
        .to_owned();
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
    use anyhow::Context as _;
    let uid = String::from_utf8(std::process::Command::new("id").arg("-u").output()?.stdout)?
        .trim()
        .to_owned();
    let _ = run_cmd(
        "launchctl",
        &["bootout", &format!("gui/{uid}/{LAUNCHD_LABEL}")],
    );
    let plist = dirs::home_dir()
        .context("cannot resolve home dir")?
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"));
    if plist.exists() {
        std::fs::remove_file(&plist)?;
    }
    println!("unregistered: {LAUNCHD_LABEL}");
    Ok(())
}

/// 등록된 LaunchAgent 재기동 — `kickstart -k`는 돌고 있으면 죽이고 다시 띄운다 (linux restart 참조).
#[cfg(target_os = "macos")]
pub fn restart() -> anyhow::Result<bool> {
    use anyhow::Context as _;
    let plist = dirs::home_dir()
        .context("cannot resolve home dir")?
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"));
    if !plist.exists() {
        return Ok(false);
    }
    let uid = String::from_utf8(std::process::Command::new("id").arg("-u").output()?.stdout)?
        .trim()
        .to_owned();
    run_cmd(
        "launchctl",
        &["kickstart", "-k", &format!("gui/{uid}/{LAUNCHD_LABEL}")],
    )?;
    Ok(true)
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

// ---------------------------------------------------------------- Windows: SCM 서비스

#[cfg(windows)]
pub fn install(config: Option<&str>) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::ffi::OsString;
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let config = require_absolute(config)?;
    let exe = std::env::current_exe()?;
    let user = std::env::var("USERNAME").context("USERNAME env")?;
    let domain = std::env::var("USERDOMAIN").unwrap_or_else(|_| ".".into());
    let account = format!("{domain}\\{user}");
    println!(
        "service account: {account} — registering under your user account, since the keychain token and the claude login for wake live in this user profile."
    );
    let password = rpassword::prompt_password(format!(
        "Windows password for {account} (stored by SCM, not echoed): "
    ))?;

    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)
            .context("failed to open the service manager — run from an administrator terminal")?;
    grant_service_logon_right(&account)?;

    let mut launch_arguments = vec![OsString::from("daemon"), OsString::from("service-run")];
    if let Some(c) = config {
        launch_arguments.push("--config".into());
        launch_arguments.push(c.into());
    }
    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: "Brevduva Receiver Daemon".into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments,
        dependencies: vec![],
        account_name: Some(account.clone().into()),
        account_password: Some(password.into()),
    };
    let service = manager
        .create_service(&info, ServiceAccess::START)
        .context("failed to create the service — if already registered, `brv daemon uninstall` and retry")?;
    service.start::<&std::ffi::OsStr>(&[]).context(
        "failed to start the service — on logon failure (1069), suspect the password you entered",
    )?;
    println!(
        "registered and started: service {SERVICE_NAME} (starts at boot, logs: daemon-service.log in the config directory)"
    );
    Ok(())
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
#[cfg(windows)]
pub fn service_run() -> anyhow::Result<()> {
    use anyhow::Context as _;
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
                ..Default::default()
            },
        )
        .await
    });
    set_status(
        &status_handle,
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        Duration::ZERO,
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
    use windows_service::service::{ServiceExitCode, ServiceStatus, ServiceType};
    handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint,
        process_id: None,
    })
}

/// `SeServiceLogonRight` 부여 — services.msc GUI와 달리 CreateService API는 이 권한을
/// 자동 부여하지 않아(문서화된 동작) 없으면 서비스 시작이 로그온 오류(1069)로 실패한다.
/// windows-sys에 안전 래퍼가 없어 LSA FFI가 불가피 — crate 전역 deny(unsafe_code)의
/// 유일한 예외 (lib.rs 주석 참조).
#[cfg(windows)]
#[allow(unsafe_code)]
fn grant_service_logon_right(account: &str) -> anyhow::Result<()> {
    use windows_sys::Win32::Security::Authentication::Identity::{
        LSA_OBJECT_ATTRIBUTES, LSA_UNICODE_STRING, LsaAddAccountRights, LsaClose,
        LsaNtStatusToWinError, LsaOpenPolicy,
    };
    use windows_sys::Win32::Security::LookupAccountNameW;

    // LsaOpenPolicy 접근 마스크 (winnt.h 문서값 — LsaAddAccountRights 요구 조합)
    const POLICY_CREATE_ACCOUNT: u32 = 0x0010;
    const POLICY_LOOKUP_NAMES: u32 = 0x0800;

    let account_w: Vec<u16> = account.encode_utf16().chain(std::iter::once(0)).collect();
    let mut sid_len = 0u32;
    let mut dom_len = 0u32;
    let mut sid_use = 0i32;
    // 1차 호출은 버퍼 크기 조회 (실패가 정상 — ERROR_INSUFFICIENT_BUFFER)
    // SAFETY: 널 버퍼 + 길이 0은 문서화된 크기 조회 규약. 포인터는 전부 지역 변수
    unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            account_w.as_ptr(),
            std::ptr::null_mut(),
            &mut sid_len,
            std::ptr::null_mut(),
            &mut dom_len,
            &mut sid_use,
        );
    }
    anyhow::ensure!(
        sid_len > 0,
        "failed to query SID size for account {account}"
    );
    let mut sid = vec![0u8; sid_len as usize];
    let mut dom = vec![0u16; dom_len as usize];
    // SAFETY: 조회된 크기만큼 할당한 버퍼를 전달 — 수명은 이 함수 내
    let ok = unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            account_w.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut sid_len,
            dom.as_mut_ptr(),
            &mut dom_len,
            &mut sid_use,
        )
    };
    anyhow::ensure!(ok != 0, "failed to look up SID for account {account}");

    // SAFETY: zeroed LSA_OBJECT_ATTRIBUTES + Length 설정은 문서화된 초기화 규약
    let mut attrs: LSA_OBJECT_ATTRIBUTES = unsafe { std::mem::zeroed() };
    attrs.Length = std::mem::size_of::<LSA_OBJECT_ATTRIBUTES>() as u32;
    let mut policy: isize = 0; // LSA_HANDLE (windows-sys 0.61에서 isize)
    // SAFETY: 로컬 시스템(SystemName=null) 정책 열기 — 성공 시 LsaClose로 반드시 닫는다
    let status = unsafe {
        LsaOpenPolicy(
            std::ptr::null(),
            &attrs,
            POLICY_CREATE_ACCOUNT | POLICY_LOOKUP_NAMES,
            &mut policy,
        )
    };
    anyhow::ensure!(
        status == 0,
        "LsaOpenPolicy failed (win32 error {})",
        unsafe { LsaNtStatusToWinError(status) }
    );

    let mut right: Vec<u16> = "SeServiceLogonRight".encode_utf16().collect();
    let lsa_right = LSA_UNICODE_STRING {
        Length: (right.len() * 2) as u16,
        MaximumLength: (right.len() * 2) as u16,
        Buffer: right.as_mut_ptr(),
    };
    // SAFETY: sid·lsa_right 버퍼는 호출 동안 유효 (지역 Vec). 이미 부여된 계정에는 멱등
    let status =
        unsafe { LsaAddAccountRights(policy, sid.as_ptr().cast_mut().cast(), &lsa_right, 1) };
    // SAFETY: 위에서 성공적으로 연 정책 핸들
    unsafe { LsaClose(policy) };
    anyhow::ensure!(
        status == 0,
        "failed to grant SeServiceLogonRight (win32 error {})",
        unsafe { LsaNtStatusToWinError(status) }
    );
    println!("service logon right (SeServiceLogonRight) granted: {account}");
    Ok(())
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
