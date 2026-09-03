// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 로그온한 사용자의 세션 안에 프로세스 띄우기 — 윈도우 시스템 서비스용 (2026-09-03,
//! 사용자 확정 "리시버는 전달자일 뿐이다 — 시스템 계정으로 가능하다면 그게 맞다").
//!
//! 구조는 백신·업데이트 에이전트와 같다: 듣는 데몬은 LocalSystem 서비스로 부팅 때부터
//! 돌고, 깨우기만 로그온한 사용자의 토큰을 빌려 **그 사용자 명의**로 띄운다 — claude
//! 로그인·프로젝트 파일·자격 증명은 전부 사용자 프로필 소속이라 서비스 계정으로는 열 수
//! 없다. 잠금 화면은 세션이 살아 있으므로 깨울 수 있고, 로그아웃 상태면 `NoUserSession`
//! 으로 관문(daemon 사전 점검)이 접속을 보류한다 — 메시지는 서버 큐에 남는다.
//!
//! 필요 권한(SeTcb·SeAssignPrimaryToken·SeIncreaseQuota)은 LocalSystem이 가진다.
//! crate 전역 `deny(unsafe_code)`의 예외 모듈 — Win32 FFI에는 안전 래퍼가 없다 (lib.rs).
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle, RawHandle};
use std::path::Path;
use std::ptr::{null, null_mut};

use anyhow::Context as _;
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GetLastError, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
    SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SECURITY_ATTRIBUTES,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows_sys::Win32::System::RemoteDesktop::{
    WTS_SESSION_INFOW, WTSActive, WTSDisconnected, WTSEnumerateSessionsW, WTSFreeMemory,
    WTSQuerySessionInformationW, WTSQueryUserToken, WTSUserName,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, GetCurrentProcess,
    GetExitCodeProcess, INFINITE, OpenProcessToken, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};

use crate::daemon::NoUserSession;

/// 사용자 세션에 띄운 자식 프로세스 — `tokio::process::Child`의 최소 대응물 (wait·kill).
pub struct Child {
    process: OwnedHandle,
    pub pid: u32,
}

impl Child {
    /// 종료 대기 — 1초 조각으로 블로킹 대기해 tokio 타임아웃과 어울린다.
    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let raw = self.process.as_raw_handle() as usize;
        loop {
            let r = tokio::task::spawn_blocking(move || {
                // SAFETY: 핸들은 self가 소유하며 이 future가 끝날 때까지 살아 있다
                unsafe { WaitForSingleObject(raw as HANDLE, 1000) }
            })
            .await
            .map_err(std::io::Error::other)?;
            if r == WAIT_OBJECT_0 {
                break;
            }
            if r != WAIT_TIMEOUT {
                return Err(std::io::Error::last_os_error());
            }
        }
        let mut code = 0u32;
        // SAFETY: 유효한 프로세스 핸들, 종료 확인 후 호출
        if unsafe { GetExitCodeProcess(self.process.as_raw_handle() as HANDLE, &mut code) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(std::os::windows::process::ExitStatusExt::from_raw(code))
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        // SAFETY: 유효한 프로세스 핸들
        if unsafe { TerminateProcess(self.process.as_raw_handle() as HANDLE, 1) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

/// 로그온한 사용자 세션에서 `program args…`를 `dir`에서 띄운다. 환경은 그 사용자의 것에
/// `extra_env`를 덧씌운 것, 표준 출력·오류는 `log`(wake.log)로, 표준 입력은 NUL.
/// `user`가 있으면 그 사용자의 세션만, 없으면 활성 세션 아무거나 (단일 사용자 머신 기본).
pub fn spawn(
    program: &str,
    args: &[String],
    dir: &str,
    extra_env: &[(&str, &str)],
    log: &std::fs::File,
    user: Option<&str>,
) -> anyhow::Result<Child> {
    enable_privileges();
    let token = user_token(user)?;
    let token_h = token.as_raw_handle() as HANDLE;

    let vars = {
        let mut block: *mut c_void = null_mut();
        // SAFETY: 유효한 사용자 토큰, 출력 포인터는 로컬
        check(
            unsafe { CreateEnvironmentBlock(&mut block, token_h, 0) },
            "CreateEnvironmentBlock",
        )?;
        // SAFETY: CreateEnvironmentBlock이 준 이중 NUL 종료 블록
        let vars = unsafe { read_block(block as *const u16) };
        // SAFETY: 위에서 받은 블록을 정확히 한 번 해제
        unsafe { DestroyEnvironmentBlock(block) };
        vars
    };
    let env = env_block(&merge_env(vars, extra_env));

    let mut cmdline = wide(&command_line(program, args));
    let dir_w = wide(dir);
    let mut desktop = wide("winsta0\\default");

    // wake.log 핸들을 상속 가능하게 복제 — 자식이 stdout/stderr로 물려받는다
    let log_h = log.try_clone().context("clone wake log handle")?;
    // SAFETY: 방금 복제한 유효한 파일 핸들
    check(
        unsafe {
            SetHandleInformation(
                log_h.as_raw_handle() as HANDLE,
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            )
        },
        "SetHandleInformation",
    )?;
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    // SAFETY: NUL 장치는 항상 존재, 인자는 전부 유효한 로컬 값
    let nul = unsafe {
        CreateFileW(
            wide("NUL").as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &sa,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    anyhow::ensure!(
        nul != INVALID_HANDLE_VALUE,
        "open NUL for stdin failed: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: 방금 받은 유효한 핸들의 소유권을 넘긴다
    let nul = unsafe { OwnedHandle::from_raw_handle(nul as RawHandle) };

    // SAFETY: STARTUPINFOW는 전부 0으로 시작해도 되는 평범한 C 구조체
    let mut si: STARTUPINFOW = unsafe { zeroed() };
    si.cb = size_of::<STARTUPINFOW>() as u32;
    si.lpDesktop = desktop.as_mut_ptr();
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdInput = nul.as_raw_handle() as HANDLE;
    si.hStdOutput = log_h.as_raw_handle() as HANDLE;
    si.hStdError = log_h.as_raw_handle() as HANDLE;
    // SAFETY: 출력 전용 구조체
    let mut pi: PROCESS_INFORMATION = unsafe { zeroed() };
    // CREATE_NO_WINDOW: 사용자 화면에 콘솔 창이 번쩍이지 않게 (헤드리스 `claude -p`)
    // SAFETY: 모든 포인터는 이 함수 안의 살아 있는 버퍼를 가리킨다
    check(
        unsafe {
            CreateProcessAsUserW(
                token_h,
                null(),
                cmdline.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
                env.as_ptr() as *const c_void,
                dir_w.as_ptr(),
                &si,
                &mut pi,
            )
        },
        "CreateProcessAsUserW",
    )?;
    // SAFETY: 스레드 핸들은 쓰지 않는다 — 즉시 반납
    unsafe { CloseHandle(pi.hThread) };
    Ok(Child {
        // SAFETY: CreateProcessAsUserW가 준 프로세스 핸들의 소유권
        process: unsafe { OwnedHandle::from_raw_handle(pi.hProcess as RawHandle) },
        pid: pi.dwProcessId,
    })
}

/// 깨울 사용자의 기본 토큰 — 활성 세션 우선, 없으면 끊긴(원격 데스크톱) 세션.
/// 잠금 화면은 "활성"이다 — 잠가도 깨울 수 있다.
fn user_token(user: Option<&str>) -> anyhow::Result<OwnedHandle> {
    let mut list: *mut WTS_SESSION_INFOW = null_mut();
    let mut count = 0u32;
    // SAFETY: 로컬 서버(null 핸들), 출력 포인터는 로컬
    check(
        unsafe { WTSEnumerateSessionsW(null_mut(), 0, 1, &mut list, &mut count) },
        "WTSEnumerateSessionsW",
    )?;
    let mut active = Vec::new();
    let mut disconnected = Vec::new();
    // SAFETY: count개의 유효한 항목
    for s in unsafe { std::slice::from_raw_parts(list, count as usize) } {
        let name = session_user(s.SessionId);
        if name.is_empty() {
            continue; // 로그온한 사용자가 없는 세션 (서비스·로그온 화면)
        }
        if s.State == WTSActive {
            active.push((s.SessionId, name));
        } else if s.State == WTSDisconnected {
            disconnected.push((s.SessionId, name));
        }
    }
    // SAFETY: WTSEnumerateSessionsW가 준 메모리를 정확히 한 번 해제
    unsafe { WTSFreeMemory(list as *mut c_void) };

    let pick = |list: &[(u32, String)]| match user {
        Some(u) => list
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(u))
            .map(|(id, _)| *id),
        None => list.first().map(|(id, _)| *id),
    };
    let Some(session) = pick(&active).or_else(|| pick(&disconnected)) else {
        let detail = match user {
            Some(u) => format!("waiting for {u} to log on"),
            None => "waiting for a user to log on".to_owned(),
        };
        return Err(NoUserSession { detail }.into());
    };
    let mut token: HANDLE = null_mut();
    // SAFETY: 유효한 세션 id, 출력 포인터는 로컬 (SeTcb 필요 — LocalSystem)
    check(
        unsafe { WTSQueryUserToken(session, &mut token) },
        "WTSQueryUserToken",
    )?;
    // SAFETY: 방금 받은 기본 토큰의 소유권
    Ok(unsafe { OwnedHandle::from_raw_handle(token as RawHandle) })
}

fn session_user(session: u32) -> String {
    let mut buf: *mut u16 = null_mut();
    let mut len = 0u32;
    // SAFETY: 로컬 서버, 출력 포인터는 로컬
    if unsafe { WTSQuerySessionInformationW(null_mut(), session, WTSUserName, &mut buf, &mut len) }
        == 0
        || buf.is_null()
    {
        return String::new();
    }
    // SAFETY: NUL 종료 UTF-16 문자열
    let name = unsafe { wide_to_string(buf) };
    // SAFETY: 위에서 받은 버퍼를 정확히 한 번 해제
    unsafe { WTSFreeMemory(buf as *mut c_void) };
    name
}

/// 서비스 토큰의 특권 활성화 — LocalSystem은 이미 가지지만 비활성일 수 있다. 실패는
/// 치명적이지 않다 (없으면 뒤의 호출이 정확한 오류를 낸다).
fn enable_privileges() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let mut token: HANDLE = null_mut();
        // SAFETY: 자기 프로세스 핸들, 출력 포인터는 로컬
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token) }
            == 0
        {
            return;
        }
        // SAFETY: 방금 받은 토큰의 소유권
        let token = unsafe { OwnedHandle::from_raw_handle(token as RawHandle) };
        for name in [
            "SeTcbPrivilege",
            "SeAssignPrimaryTokenPrivilege",
            "SeIncreaseQuotaPrivilege",
        ] {
            // SAFETY: 평범한 C 구조체
            let mut tp: TOKEN_PRIVILEGES = unsafe { zeroed() };
            tp.PrivilegeCount = 1;
            tp.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
            // SAFETY: 유효한 이름 버퍼와 출력 포인터
            if unsafe {
                LookupPrivilegeValueW(null(), wide(name).as_ptr(), &mut tp.Privileges[0].Luid)
            } == 0
            {
                continue;
            }
            // SAFETY: 유효한 토큰과 구조체
            unsafe {
                AdjustTokenPrivileges(
                    token.as_raw_handle() as HANDLE,
                    0,
                    &tp,
                    0,
                    null_mut(),
                    null_mut(),
                )
            };
        }
    });
}

/// 이 프로세스가 관리자 권한(승격)으로 도는가 — `brv daemon install`의 UAC 자기 승격 판단
/// (2026-09-04, 온보딩 재설계 2).
pub fn is_elevated() -> bool {
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    let mut token: HANDLE = null_mut();
    // SAFETY: 자기 프로세스 핸들, 출력 포인터는 로컬
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    // SAFETY: 방금 받은 토큰의 소유권
    let token = unsafe { OwnedHandle::from_raw_handle(token as RawHandle) };
    // SAFETY: 평범한 C 구조체
    let mut elevation: TOKEN_ELEVATION = unsafe { zeroed() };
    let mut len = 0u32;
    // SAFETY: 유효한 토큰, 구조체 크기만큼의 출력 버퍼
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle() as HANDLE,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut c_void,
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    ok != 0 && elevation.TokenIsElevated != 0
}

/// 같은 실행 파일을 관리자 권한으로 다시 실행하고 끝나기를 기다린다 — UAC 창이 한 번 뜬다.
/// 승격된 프로세스의 출력은 이 콘솔로 돌아오지 않으므로 `cmd /c … > log 2>&1`로 파일에 받고,
/// 호출자가 그 파일을 보여준다. 반환값은 종료 코드. UAC를 취소하면 Err.
pub fn run_elevated(exe: &Path, args: &[String], log: &Path) -> anyhow::Result<i32> {
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    let inner = command_line(&exe.to_string_lossy(), args);
    // cmd의 인용 규칙: 전체를 한 쌍의 따옴표로 감싸면 안쪽 따옴표를 그대로 보존한다
    let params = wide(&format!("/d /c \"{inner} > \"{}\" 2>&1\"", log.display()));
    let verb = wide("runas");
    let file = wide("cmd.exe");
    // SAFETY: 평범한 C 구조체
    let mut info: SHELLEXECUTEINFOW = unsafe { zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = 0; // SW_HIDE — 승격된 cmd 창은 띄우지 않는다 (UAC 창은 별도로 뜬다)
    // SAFETY: 모든 포인터는 이 함수 안의 살아 있는 버퍼를 가리킨다
    check(
        unsafe { ShellExecuteExW(&mut info) },
        "ShellExecuteExW (the administrator prompt was declined or failed)",
    )?;
    anyhow::ensure!(!info.hProcess.is_null(), "elevated process handle missing");
    // SAFETY: SEE_MASK_NOCLOSEPROCESS로 받은 프로세스 핸들의 소유권
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess as RawHandle) };
    // SAFETY: 유효한 프로세스 핸들
    unsafe { WaitForSingleObject(process.as_raw_handle() as HANDLE, INFINITE) };
    let mut code = 0u32;
    // SAFETY: 유효한 프로세스 핸들, 종료 확인 후 호출
    check(
        unsafe { GetExitCodeProcess(process.as_raw_handle() as HANDLE, &mut code) },
        "GetExitCodeProcess",
    )?;
    Ok(code as i32)
}

fn check(ok: i32, what: &str) -> anyhow::Result<()> {
    if ok == 0 {
        // SAFETY: 실패 직후 호출
        let code = unsafe { GetLastError() };
        anyhow::bail!(
            "{what} failed: {} (win32 error {code})",
            std::io::Error::from_raw_os_error(code as i32)
        );
    }
    Ok(())
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// # Safety
/// `p`는 NUL 종료 UTF-16 문자열을 가리켜야 한다.
unsafe fn wide_to_string(p: *const u16) -> String {
    let mut len = 0usize;
    // SAFETY: 호출자 계약 — NUL까지 읽을 수 있다
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: 위에서 잰 길이만큼 유효
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

/// # Safety
/// `block`은 이중 NUL로 끝나는 `이름=값` UTF-16 블록이어야 한다 (CreateEnvironmentBlock).
unsafe fn read_block(block: *const u16) -> Vec<(String, String)> {
    let mut vars = Vec::new();
    let mut p = block;
    loop {
        // SAFETY: 호출자 계약 — 이중 NUL 전까지 유효
        let s = unsafe { wide_to_string(p) };
        if s.is_empty() {
            break;
        }
        let len = s.encode_utf16().count() + 1;
        // 드라이브별 cwd 항목(`=C:=C:\…`)은 첫 글자가 '='라 두 번째 '='에서 가른다
        if let Some(i) = s[1..].find('=') {
            let (k, v) = s.split_at(i + 1);
            vars.push((k.to_owned(), v[1..].to_owned()));
        }
        // SAFETY: 다음 문자열의 시작 (이중 NUL이면 빈 문자열에서 멈춘다)
        p = unsafe { p.add(len) };
    }
    vars
}

/// 사용자 환경에 데몬의 변수를 덧씌운다 — 윈도우 환경 변수 이름은 대소문자 무시.
fn merge_env(mut vars: Vec<(String, String)>, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    for (k, v) in extra {
        match vars
            .iter_mut()
            .find(|(name, _)| name.eq_ignore_ascii_case(k))
        {
            Some(slot) => slot.1 = (*v).to_owned(),
            None => vars.push(((*k).to_owned(), (*v).to_owned())),
        }
    }
    vars
}

/// `이름=값\0…\0\0` UTF-16 블록 (CREATE_UNICODE_ENVIRONMENT).
fn env_block(vars: &[(String, String)]) -> Vec<u16> {
    let mut out = Vec::new();
    for (k, v) in vars {
        out.extend(format!("{k}={v}").encode_utf16());
        out.push(0);
    }
    if vars.is_empty() {
        out.push(0);
    }
    out.push(0);
    out
}

/// CreateProcess 명령줄 — Rust 표준 라이브러리의 `Command`와 같은 인용 규칙(MSVCRT 호환):
/// 프로그램은 항상 따옴표, 인자는 공백·탭·빈 값일 때 따옴표, 따옴표 앞 백슬래시는 배가.
fn command_line(program: &str, args: &[String]) -> String {
    let mut out = String::new();
    append_arg(&mut out, program, true);
    for a in args {
        out.push(' ');
        append_arg(&mut out, a, false);
    }
    out
}

fn append_arg(out: &mut String, arg: &str, force_quote: bool) {
    let quote = force_quote || arg.is_empty() || arg.contains([' ', '\t']);
    if quote {
        out.push('"');
    }
    let mut backslashes = 0usize;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                // 내부 따옴표 앞: 2n+1개의 백슬래시
                out.extend(std::iter::repeat_n('\\', backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(c);
    }
    if quote {
        // 닫는 따옴표 앞: 2n개의 백슬래시
        out.extend(std::iter::repeat_n('\\', backslashes));
        out.push('"');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_quotes_like_std() {
        let args = vec![
            "-p".to_owned(),
            "say \"hi\" now".to_owned(),
            r"C:\dir\".to_owned(),
            String::new(),
        ];
        assert_eq!(
            command_line(r"C:\Program Files\claude.exe", &args),
            r#""C:\Program Files\claude.exe" -p "say \"hi\" now" C:\dir\ """#
        );
    }

    #[test]
    fn merge_env_overrides_case_insensitively() {
        let vars = vec![
            ("Path".to_owned(), "x".to_owned()),
            ("brevduva_config".to_owned(), "old".to_owned()),
        ];
        let merged = merge_env(
            vars,
            &[("BREVDUVA_CONFIG", "new"), ("BREVDUVA_BINDING", "a@b")],
        );
        assert_eq!(
            merged,
            vec![
                ("Path".to_owned(), "x".to_owned()),
                ("brevduva_config".to_owned(), "new".to_owned()),
                ("BREVDUVA_BINDING".to_owned(), "a@b".to_owned()),
            ]
        );
        let block = env_block(&merged);
        assert_eq!(
            block.len(),
            "Path=x\0brevduva_config=new\0BREVDUVA_BINDING=a@b\0\0".len()
        );
        assert_eq!(block[block.len() - 2..], [0, 0]);
    }

    #[test]
    fn read_block_splits_drive_cwd_entries() {
        let raw: Vec<u16> = "=C:=C:\\work\0A=1\0\0".encode_utf16().collect();
        // SAFETY: 테스트 버퍼는 이중 NUL로 끝난다
        let vars = unsafe { read_block(raw.as_ptr()) };
        assert_eq!(
            vars,
            vec![
                ("=C:".to_owned(), "C:\\work".to_owned()),
                ("A".to_owned(), "1".to_owned()),
            ]
        );
    }
}
