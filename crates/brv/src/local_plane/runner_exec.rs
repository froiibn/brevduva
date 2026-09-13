// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 러너 실행기 — 러너 입력 통로에 넣는 명령을 **로그온 사용자 명의로** 실행한다
//! (RECEIVER_DESIGN.md P5·P9, 2026-09-10).
//!
//! 왜 따로 두는가: `codex queue` 같은 실행체는 사용자 프로필 안에서 설정을 읽고 파일을 만든다.
//! 윈도우 리시버는 LocalSystem 서비스라 그대로 돌리면 사용자 폴더에 SYSTEM 명의 파일이 생기고,
//! 환경(USERPROFILE 등)도 SYSTEM의 것이 된다(2026-09-10 실측, openai/codex `rust-v0.153.4` 소스
//! 확인). 그래서 윈도우 서비스 모드는 깨우기와 같은 winspawn으로 사용자 세션에 띄우고, 리눅스·맥은
//! 서비스 자체가 사용자라 직접 실행한다 — 세 OS에서 "사용자 명의" 규칙이 같다.
//!
//! 트레이트로 두는 이유: 평면의 시험이 실제 러너 없이 전달 경로를 검증한다. 실행 결과의 해석
//! (queue id 등)은 부르는 쪽이 한다 — 이 모듈은 러너를 모른다(로봇 제어기 명령도 같은 자리에 온다).

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context as _;

use crate::daemon::WakeSpawn;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 환경 변수 — 값이 `None`이면 지운다.
pub type EnvVar = (String, Option<String>);

/// 한 번의 실행 결과. 표준 출력과 오류를 합친 글이다 — 사용자 세션 실행은 핸들 하나로 받는다.
#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub exit_success: bool,
    pub exit_code: Option<i32>,
    /// 상한 안에 끝나지 않아 끊었다 — 결과를 모른다.
    pub timed_out: bool,
    pub output: String,
}

impl ExecOutput {
    /// 끝까지 돌았고 성공으로 끝났다.
    pub fn succeeded(&self) -> bool {
        !self.timed_out && self.exit_success
    }

    #[cfg(test)]
    pub(crate) fn exited(success: bool, output: &str) -> Self {
        Self {
            exit_success: success,
            exit_code: Some(if success { 0 } else { 1 }),
            timed_out: false,
            output: output.to_owned(),
        }
    }

    #[cfg(test)]
    pub(crate) fn timed_out(output: &str) -> Self {
        Self {
            exit_success: false,
            exit_code: None,
            timed_out: true,
            output: output.to_owned(),
        }
    }
}

pub trait RunnerExec: Send + Sync + 'static {
    /// `program args…`를 `dir`에서 실행한다. 시작조차 못 하면 `Err`, 끝났거나 끊었으면 `Ok`.
    fn run<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
        env: &'a [EnvVar],
        dir: &'a Path,
        timeout: Duration,
    ) -> BoxFuture<'a, anyhow::Result<ExecOutput>>;
}

/// 제품 실행기 — 데몬의 깨우기와 같은 명의로 실행한다.
pub struct UserContextExec {
    spawn: WakeSpawn,
    /// 사용자 세션 실행의 출력을 받을 임시 파일 자리 (윈도우 서비스 모드만 쓴다).
    scratch: PathBuf,
}

impl UserContextExec {
    pub fn new(spawn: WakeSpawn, scratch: PathBuf) -> Self {
        Self { spawn, scratch }
    }
}

impl RunnerExec for UserContextExec {
    fn run<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
        env: &'a [EnvVar],
        dir: &'a Path,
        timeout: Duration,
    ) -> BoxFuture<'a, anyhow::Result<ExecOutput>> {
        Box::pin(async move {
            match &self.spawn {
                WakeSpawn::Direct => run_direct(program, args, env, dir, timeout).await,
                WakeSpawn::UserSession { user } => {
                    run_in_user_session(
                        &self.scratch,
                        program,
                        args,
                        env,
                        dir,
                        timeout,
                        user.as_deref(),
                    )
                    .await
                }
            }
        })
    }
}

/// 이 프로세스의 명의로 — 리눅스·맥 서비스(사용자 유닛·LaunchAgent)와 포그라운드 실행.
async fn run_direct(
    program: &Path,
    args: &[String],
    env: &[EnvVar],
    dir: &Path,
    timeout: Duration,
) -> anyhow::Result<ExecOutput> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in env {
        match value {
            Some(value) => {
                command.env(key, value);
            }
            None => {
                command.env_remove(key);
            }
        }
    }
    // CREATE_NO_WINDOW: 사용자 화면에 콘솔 창이 번쩍이지 않게
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let child = command
        .spawn()
        .with_context(|| format!("cannot start {program:?}"))?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(output) => {
            let output = output?;
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            Ok(ExecOutput {
                exit_success: output.status.success(),
                exit_code: output.status.code(),
                timed_out: false,
                output: text,
            })
        }
        // 기다리던 future가 버려지며 자식도 버려진다 — kill_on_drop이 끊는다.
        Err(_) => Ok(ExecOutput {
            exit_success: false,
            exit_code: None,
            timed_out: true,
            output: String::new(),
        }),
    }
}

/// 윈도우 LocalSystem 서비스 — 로그온한 사용자의 세션에 그 사용자 명의로(winspawn). 출력은
/// 상속 핸들로 임시 파일에 받는다. 사용자 환경 블록은 로그온 환경이라 지울 변수가 애초에 없다 —
/// `None` 항목은 덧씌우지 않는다.
#[cfg(windows)]
async fn run_in_user_session(
    scratch: &Path,
    program: &Path,
    args: &[String],
    env: &[EnvVar],
    dir: &Path,
    timeout: Duration,
    user: Option<&str>,
) -> anyhow::Result<ExecOutput> {
    std::fs::create_dir_all(scratch)?;
    let path = scratch.join(format!(
        "exec-{}.log",
        brevduva_protocol::ClientKey::generate()
    ));
    let log = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("cannot create {path:?}"))?;
    let pinned: Vec<(&str, &str)> = env
        .iter()
        .filter_map(|(key, value)| value.as_deref().map(|value| (key.as_str(), value)))
        .collect();
    let started = crate::winspawn::spawn(
        &program.to_string_lossy(),
        args,
        &dir.to_string_lossy(),
        &pinned,
        &log,
        None, // 러너 입력 도우미는 표준 입력을 쓰지 않는다
        user,
    );
    drop(log);
    let mut child = match started {
        Ok(child) => child,
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            return Err(error).with_context(|| {
                format!("cannot start {program:?} in the logged-on user's session")
            });
        }
    };
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => Some(status?),
        Err(_) => {
            let _ = child.kill();
            None
        }
    };
    let output = std::fs::read(&path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    Ok(match status {
        Some(status) => ExecOutput {
            exit_success: status.success(),
            exit_code: status.code(),
            timed_out: false,
            output,
        },
        None => ExecOutput {
            exit_success: false,
            exit_code: None,
            timed_out: true,
            output,
        },
    })
}

#[cfg(not(windows))]
async fn run_in_user_session(
    scratch: &Path,
    program: &Path,
    args: &[String],
    env: &[EnvVar],
    dir: &Path,
    timeout: Duration,
    user: Option<&str>,
) -> anyhow::Result<ExecOutput> {
    let _ = (scratch, program, args, env, dir, timeout, user);
    anyhow::bail!("user-session execution is Windows-only (service mode)")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> (PathBuf, Vec<String>) {
        #[cfg(windows)]
        {
            (PathBuf::from("cmd"), vec!["/C".into(), script.into()])
        }
        #[cfg(not(windows))]
        {
            (PathBuf::from("sh"), vec!["-c".into(), script.into()])
        }
    }

    fn direct() -> UserContextExec {
        UserContextExec::new(WakeSpawn::Direct, std::env::temp_dir())
    }

    #[tokio::test]
    async fn direct_execution_returns_the_exit_status_and_combined_output() {
        let (program, args) = shell("echo brv-exec-out && echo brv-exec-err 1>&2 && exit 3");
        let out = direct()
            .run(
                &program,
                &args,
                &[],
                &std::env::temp_dir(),
                Duration::from_secs(20),
            )
            .await
            .expect("started");
        assert!(!out.succeeded());
        assert!(!out.timed_out);
        assert_eq!(out.exit_code, Some(3));
        assert!(out.output.contains("brv-exec-out"), "{}", out.output);
        assert!(out.output.contains("brv-exec-err"), "{}", out.output);
    }

    #[tokio::test]
    async fn direct_execution_pins_the_given_environment() {
        let script = if cfg!(windows) {
            "echo [%BRV_EXEC_PIN%]"
        } else {
            "echo [$BRV_EXEC_PIN]"
        };
        let (program, args) = shell(script);
        let env = vec![("BRV_EXEC_PIN".to_owned(), Some("pinned".to_owned()))];
        let out = direct()
            .run(
                &program,
                &args,
                &env,
                &std::env::temp_dir(),
                Duration::from_secs(20),
            )
            .await
            .expect("started");
        assert!(out.succeeded());
        assert!(out.output.contains("[pinned]"), "{}", out.output);
    }

    #[tokio::test]
    async fn a_command_that_does_not_finish_is_cut_off_and_reported() {
        let script = if cfg!(windows) {
            "ping -n 4 127.0.0.1 > NUL"
        } else {
            "sleep 4"
        };
        let (program, args) = shell(script);
        let started = std::time::Instant::now();
        let out = direct()
            .run(
                &program,
                &args,
                &[],
                &std::env::temp_dir(),
                Duration::from_millis(300),
            )
            .await
            .expect("started");
        assert!(out.timed_out, "결과를 모르면 모른다고 돌려준다");
        assert!(!out.succeeded());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn a_program_that_cannot_start_is_an_error_not_an_outcome() {
        let result = direct()
            .run(
                Path::new("brv-no-such-program-for-exec-test"),
                &[],
                &[],
                &std::env::temp_dir(),
                Duration::from_secs(5),
            )
            .await;
        assert!(result.is_err());
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn user_session_execution_is_refused_off_windows() {
        let exec =
            UserContextExec::new(WakeSpawn::UserSession { user: None }, std::env::temp_dir());
        let (program, args) = shell("true");
        assert!(
            exec.run(
                &program,
                &args,
                &[],
                &std::env::temp_dir(),
                Duration::from_secs(5)
            )
            .await
            .is_err()
        );
    }
}
