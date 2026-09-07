// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 사용자의 '현재 작업에 연결' 표면. 로컬 세션 식별과 실행 어댑터를 분리한다.
//! 연결 의도는 디스크에 남기고 사용자 계정의 백그라운드 worker가 실제 수신을 맡는다.
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use brevduva_protocol::ClientKey;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{self, Binding, BrvConfig};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Task {
    adapter: String,
    id: String,
}

fn current_task(codex_id: Option<String>) -> anyhow::Result<Task> {
    let id = codex_id.filter(|s| !s.is_empty()).context(
        "current task identity is unavailable; run `brv connect` from this task's own shell. Do not select a recent/focused task or ask the user to copy an ID. This host may not forward session identity to its MCP process",
    )?;
    anyhow::ensure!(
        id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "invalid host session identity"
    );
    Ok(Task {
        adapter: "codex-desktop".into(),
        id,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Desired {
    Connected,
    Paused,
    Disconnected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Connection {
    server: String,
    binding: String,
    task: Task,
    generation: String,
    desired: Desired,
}

#[derive(Serialize, Deserialize)]
struct RuntimeState {
    generation: String,
    state: String,
    detail: Option<String>,
    updated: u64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn directory(binding: &Binding) -> anyhow::Result<PathBuf> {
    Ok(crate::desktop::journal_path(binding)?
        .parent()
        .context("journal directory")?
        .to_path_buf())
}
fn read(dir: &Path, cfg: &BrvConfig, binding: &Binding) -> anyhow::Result<Option<Connection>> {
    let path = dir.join("connection.json");
    if !path.exists() {
        return Ok(None);
    }
    let connection: Connection = serde_json::from_slice(&std::fs::read(path)?)?;
    anyhow::ensure!(
        connection.server == cfg.server && connection.binding == binding.full_label(),
        "saved connection identity mismatch"
    );
    anyhow::ensure!(
        connection.task.adapter == "codex-desktop",
        "unsupported saved connection adapter"
    );
    Ok(Some(connection))
}
fn write<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", ClientKey::generate()));
    let result = (|| -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
fn lock(path: &Path) -> anyhow::Result<crate::file_lock::FileLock> {
    crate::file_lock::FileLock::acquire(path).context("cannot acquire connection lock")
}
fn worker_alive(dir: &Path) -> anyhow::Result<bool> {
    crate::file_lock::FileLock::held(&dir.join("worker.lock"))
}
pub(crate) fn recovery_guard(
    cfg: &BrvConfig,
    binding: &Binding,
) -> anyhow::Result<crate::file_lock::FileLock> {
    let dir = directory(binding)?;
    let guard = lock(&dir.join("control.lock"))?;
    anyhow::ensure!(
        !worker_alive(&dir)?,
        "pause the task connection before resolving a delivery"
    );
    anyhow::ensure!(
        read(&dir, cfg, binding)?.is_none_or(|c| c.desired != Desired::Connected),
        "pause the saved connection before resolving a delivery"
    );
    Ok(guard)
}
fn runtime(
    dir: &Path,
    connection: &Connection,
    state: &str,
    detail: Option<String>,
) -> anyhow::Result<()> {
    write(
        &dir.join("runtime.json"),
        &RuntimeState {
            generation: connection.generation.clone(),
            state: state.into(),
            detail,
            updated: now(),
        },
    )
}
fn show(dir: &Path, connection: Option<&Connection>) -> anyhow::Result<()> {
    let Some(connection) = connection else {
        println!(
            "{}",
            json!({"status":"disconnected","adapter":"codex-desktop","scope":"saved_desktop_connection","message":"no Desktop task connected; this is independent of MCP tool access or CLI session delivery"})
        );
        return Ok(());
    };
    let saved: Option<RuntimeState> = std::fs::read(dir.join("runtime.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let alive = worker_alive(dir)?;
    let state = match connection.desired {
        Desired::Paused => "paused",
        Desired::Disconnected => "disconnected",
        Desired::Connected if !alive => saved
            .as_ref()
            .filter(|s| s.generation == connection.generation && s.state == "needs_attention")
            .map_or("stopped", |_| "needs_attention"),
        Desired::Connected => saved
            .as_ref()
            .filter(|s| s.generation == connection.generation)
            .map_or("starting", |s| s.state.as_str()),
    };
    println!(
        "{}",
        json!({"scope":"saved_desktop_connection","binding":connection.binding,"adapter":connection.task.adapter,"task_id":connection.task.id,
        "status":state,"worker_running":alive,"detail":saved.filter(|s| s.generation == connection.generation).and_then(|s| s.detail)})
    );
    Ok(())
}
async fn wait_stopped(dir: &Path) -> anyhow::Result<()> {
    for _ in 0..100 {
        if !worker_alive(dir)? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("receiver is still stopping; inspect connection status before retrying")
}
fn spawn_worker(
    binding: &Binding,
    connection: &Connection,
    dir: &Path,
) -> anyhow::Result<std::process::Child> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("worker.log"))?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .args([
            "connection",
            "worker",
            "--binding",
            &binding.full_label(),
            "--generation",
            &connection.generation,
        ])
        .env("BREVDUVA_CONFIG", config::config_path()?)
        .env("RUST_LOG", "info")
        .env_remove("BREVDUVA_BINDING")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW: 사용자에게 콘솔 창을 띄우지 않는다.
    }
    let child = command
        .spawn()
        .context("could not start background task receiver")?;
    Ok(child)
}

async fn wait_ready(
    dir: &Path,
    connection: &Connection,
    child: &mut std::process::Child,
    allow_app_wait: bool,
) -> anyhow::Result<()> {
    for _ in 0..100 {
        anyhow::ensure!(
            child.try_wait()?.is_none(),
            "task receiver exited during startup; inspect connection status and worker.log"
        );
        if let Ok(bytes) = std::fs::read(dir.join("runtime.json")) {
            let state: RuntimeState = serde_json::from_slice(&bytes)?;
            if state.generation == connection.generation {
                anyhow::ensure!(
                    state.state != "needs_attention",
                    "task receiver failed: {}",
                    state.detail.unwrap_or_default()
                );
                if matches!(state.state.as_str(), "receiving" | "standby")
                    || (allow_app_wait && state.state == "waiting_for_app")
                {
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!(
        "task receiver is not ready after 10 seconds; startup may continue in background — inspect connection status"
    )
}

/// Installer restart: only saved connected bindings, preserving paused/disconnected intent.
pub async fn restart_connected(cfg: &BrvConfig) -> anyhow::Result<()> {
    let mut failures = Vec::new();
    for binding in &cfg.bindings {
        let result = async {
            let dir = directory(binding)?;
            if !dir.exists() {
                return Ok::<(), anyhow::Error>(());
            }
            let _guard = lock(&dir.join("control.lock"))?;
            let Some(mut connection) = read(&dir, cfg, binding)? else {
                return Ok(());
            };
            if connection.desired != Desired::Connected {
                return Ok(());
            }
            // Fence the previous worker without changing the user's saved intent.
            connection.generation = ClientKey::generate().to_string();
            write(&dir.join("connection.json"), &connection)?;
            wait_stopped(&dir).await?;
            crate::desktop::validate_saved(cfg, binding, &connection.task.id)?;
            runtime(&dir, &connection, "starting", None)?;
            let mut child = spawn_worker(binding, &connection, &dir)?;
            wait_ready(&dir, &connection, &mut child, true).await?;
            show(&dir, Some(&connection))
        }
        .await;
        if let Err(error) = result {
            failures.push(format!("{}: {error:#}", binding.full_label()));
        }
    }
    anyhow::ensure!(
        failures.is_empty(),
        "task receiver restart failures:\n{}",
        failures.join("\n")
    );
    Ok(())
}

pub async fn command(
    cfg: &BrvConfig,
    binding: &Binding,
    action: &str,
    replace: bool,
) -> anyhow::Result<()> {
    let dir = directory(binding)?;
    if action == "status" {
        return show(&dir, read(&dir, cfg, binding)?.as_ref());
    }
    std::fs::create_dir_all(&dir)?;
    config::restrict_dir(&dir)?;
    let _command_lock = lock(&dir.join("control.lock"))?;
    let previous = read(&dir, cfg, binding)?;
    if action == "connect" || action == "resume" {
        let task = if action == "connect" {
            current_task(std::env::var("CODEX_THREAD_ID").ok())?
        } else {
            previous
                .as_ref()
                .context("no saved connection; ask this task to connect first")?
                .task
                .clone()
        };
        // CODEX_THREAD_ID가 있어도 별도 CLI 실행체일 수 있다. 실제 owner 검증이 필수다.
        crate::desktop::probe(&task.id).await.context("this task has no reachable supported Desktop owner; CLI/other-runner delivery is not yet supported")?;
        if let Some(old) = &previous {
            anyhow::ensure!(
                old.task == task || old.desired == Desired::Disconnected || replace,
                "another task is connected ({}) — ask the user whether to switch, then use --replace",
                old.task.id
            );
            if old.task == task && old.desired == Desired::Connected && worker_alive(&dir)? {
                let state: RuntimeState =
                    serde_json::from_slice(&std::fs::read(dir.join("runtime.json"))?)?;
                anyhow::ensure!(
                    state.generation == old.generation
                        && matches!(state.state.as_str(), "receiving" | "standby"),
                    "existing receiver is not ready ({}); inspect connection status",
                    state.state
                );
                return show(&dir, Some(old));
            }
            if worker_alive(&dir)? {
                let mut stopped = old.clone();
                stopped.desired = Desired::Paused;
                write(&dir.join("connection.json"), &stopped)?;
                wait_stopped(&dir).await?;
            }
        }
        crate::desktop::validate_saved(cfg, binding, &task.id)?;
        let connection = Connection {
            server: cfg.server.clone(),
            binding: binding.full_label(),
            task,
            generation: ClientKey::generate().to_string(),
            desired: Desired::Connected,
        };
        write(&dir.join("connection.json"), &connection)?;
        runtime(&dir, &connection, "starting", None)?;
        let mut child = spawn_worker(binding, &connection, &dir)?;
        wait_ready(&dir, &connection, &mut child, false).await?;
        show(&dir, Some(&connection))
    } else {
        let mut connection = previous.context("no saved task connection")?;
        connection.desired = match action {
            "pause" => Desired::Paused,
            "disconnect" => Desired::Disconnected,
            _ => anyhow::bail!("unknown connection action"),
        };
        write(&dir.join("connection.json"), &connection)?;
        wait_stopped(&dir).await?;
        show(&dir, Some(&connection))
    }
}

fn enabled(saved: &Connection, generation: &str) -> bool {
    saved.generation == generation && saved.desired == Desired::Connected
}
async fn stop_signal(
    dir: &Path,
    cfg: &BrvConfig,
    binding: &Binding,
    generation: &str,
) -> anyhow::Result<()> {
    loop {
        if !read(dir, cfg, binding)?.is_some_and(|s| enabled(&s, generation)) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

pub async fn worker(selector: &str, generation: &str) -> anyhow::Result<()> {
    // connect가 worker 준비를 확인하고 반환하기 전에 제어 터미널에서 분리한다.
    // 새 프로세스 안에서 호출하므로 pre_exec의 unsafe/멀티스레드 제약이 없다.
    #[cfg(unix)]
    rustix::process::setsid().context("cannot detach task receiver from terminal")?;
    let cfg = config::load()?;
    let binding = cfg.select(Some(selector))?;
    let dir = directory(binding)?;
    let _worker_lock = lock(&dir.join("worker.lock"))?;
    let connection = read(&dir, &cfg, binding)?.context("no saved task connection")?;
    if !enabled(&connection, generation) {
        return Ok(());
    }
    let outcome = async {
        loop {
            crate::desktop::validate_saved(&cfg, binding, &connection.task.id)?;
            if let Err(error) = crate::desktop::probe(&connection.task.id).await {
                runtime(
                    &dir,
                    &connection,
                    "waiting_for_app",
                    Some(error.to_string()),
                )?;
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
            let token = config::load_token(&cfg, binding)?;
            let mut opts = crate::client::ClientOptions::new(
                &cfg.server,
                &binding.channel,
                &binding.agent,
                token,
            );
            opts.description = binding.description.clone();
            runtime(&dir, &connection, "starting_receiver", None)?;
            // native run이 반환하면 오류를 명확히 보인다. 불명확한 입력을 자동 재전송하지 않는다.
            let observe = |state: &crate::client::ClientState| {
                use crate::client::ClientState;
                let label = match state {
                    ClientState::Connected => "receiving",
                    ClientState::Standby => "standby",
                    ClientState::Stopped { .. } | ClientState::Suspended { .. } => {
                        "needs_attention"
                    }
                    _ => "connecting",
                };
                runtime(
                    &dir,
                    &connection,
                    label,
                    Some(serde_json::to_string(state)?),
                )
            };
            let result = crate::desktop::run_observed(
                &cfg,
                binding,
                opts,
                &connection.task.id,
                None,
                &observe,
            )
            .await;
            if result.is_err() && crate::desktop::probe(&connection.task.id).await.is_err() {
                // 전송 결과가 불명확하면 validate_saved가 재시작을 막는다.
                crate::desktop::validate_saved(&cfg, binding, &connection.task.id)?;
                runtime(
                    &dir,
                    &connection,
                    "waiting_for_app",
                    result.as_ref().err().map(ToString::to_string),
                )?;
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
            return result;
        }
    };
    tokio::select! {
        result = outcome => {
            runtime(&dir, &connection, if result.is_err() {"needs_attention"} else {"stopped"}, result.as_ref().err().map(ToString::to_string))?;
            result
        }
        result = stop_signal(&dir, &cfg, binding, generation) => {
            runtime(&dir, &connection, "stopped", None)?;
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn exited_worker_is_not_successful_startup() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child.wait().unwrap();
        let connection = Connection {
            server: "s".into(),
            binding: "a@c".into(),
            task: current_task(Some("t".into())).unwrap(),
            generation: "g".into(),
            desired: Desired::Connected,
        };
        assert!(
            wait_ready(Path::new("unused"), &connection, &mut child, false)
                .await
                .is_err()
        );
    }
    #[test]
    fn current_task_requires_host_identity() {
        assert!(current_task(None).is_err());
        assert!(current_task(Some("../latest".into())).is_err());
        assert_eq!(
            current_task(Some("current-123".into())).unwrap().id,
            "current-123"
        );
    }
    #[test]
    fn old_workers_and_paused_connections_cannot_continue() {
        let mut saved = Connection {
            server: "s".into(),
            binding: "a@c".into(),
            task: current_task(Some("t".into())).unwrap(),
            generation: "new".into(),
            desired: Desired::Connected,
        };
        assert!(enabled(&saved, "new"));
        assert!(!enabled(&saved, "old"));
        saved.desired = Desired::Paused;
        assert!(!enabled(&saved, "new"));
        saved.desired = Desired::Disconnected;
        assert!(!enabled(&saved, "new"));
    }
    #[test]
    fn atomic_settings_remain_readable_under_worker_lock() {
        let dir = std::env::temp_dir().join(format!("brv-connection-{}", ClientKey::generate()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("state.json");
        write(&path, &json!({"state":"connected"})).unwrap();
        let guard = lock(&dir.join("worker.lock")).unwrap();
        assert!(worker_alive(&dir).unwrap());
        write(&path, &json!({"state":"paused"})).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap()["state"],
            "paused"
        );
        drop(guard);
        assert!(!worker_alive(&dir).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
