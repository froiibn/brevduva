//! brv — Brevduva 리시버 CLI.
//!
//! `init`(셋업 일괄) · `status` · `send` · `listen`(수신 출력) · `mcp`(로컬 MCP 서버).
//! 데몬화(OS 서비스 등록·idle 세션 깨우기)는 후속 — IMPLEMENTATION.md 잔여 참조.

use std::time::Duration;

use anyhow::Context as _;
use brv::client::{Client, ClientOptions, PublishSpec, RecvFilter};
use brv::config::{self, BrvConfig};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "brv",
    version,
    about = "Brevduva receiver & CLI — real-time messaging for AI agents"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 에이전트 연결 — 일회용 코드(--enroll, 권장) 또는 관리 API 키로
    Init {
        /// 서버 베이스 URL (예: https://api.brevduva.dev)
        #[arg(long)]
        server: String,
        /// 일회용 연결 코드 (대시보드 "머신 연결"에서 발급) — 관리 키 불필요
        #[arg(long)]
        enroll: Option<String>,
        /// 관리 API 키 (BREVDUVA_ADMIN_KEY) — --enroll 사용 시 불필요
        #[arg(
            long,
            env = "BREVDUVA_ADMIN_KEY",
            required_unless_present = "enroll",
            conflicts_with = "enroll"
        )]
        admin_key: Option<String>,
        /// 이 머신의 에이전트 이름 (예: backend) — enroll에서는 코드가 결정
        #[arg(long, required_unless_present = "enroll", conflicts_with = "enroll")]
        agent: Option<String>,
        /// 채널(프로젝트) 이름 — enroll에서는 부여된 채널 중 선택(생략 시 첫 채널)
        #[arg(long, required_unless_present = "enroll")]
        channel: Option<String>,
        /// 능력 선언 소개문 — 동료 에이전트의 라우팅 판단 근거 (enroll은 발급 시 값 사용)
        #[arg(long, default_value = "")]
        description: String,
        /// 에이전트가 이미 있으면 토큰을 회전해 재사용
        #[arg(long, conflicts_with = "enroll")]
        rotate: bool,
        /// enroll 후 Claude Code MCP 자동 등록을 생략
        #[arg(long)]
        no_mcp: bool,
    },
    /// 설정·서버·채널 상태 점검
    Status,
    /// 메시지 한 건 발행 (수동 테스트용)
    Send {
        #[arg(long)]
        to: String,
        #[arg(long)]
        payload: String,
        /// broadcast에 ack 수집을 요구 (11장)
        #[arg(long)]
        expects_ack: bool,
    },
    /// 수신 메시지를 줄 단위 JSON으로 출력 (수동 테스트용, Ctrl+C로 종료)
    Listen,
    /// 로컬 MCP 서버 (stdio) — Claude Code 등 MCP 호스트용
    Mcp,
    /// 상주 데몬 — 메시지 도착 시 세션을 깨워 처리 (config의 [wake] 필요)
    Daemon {
        #[command(subcommand)]
        action: Option<DaemonCmd>,
    },
}

/// `brv daemon`의 서비스 등록 서브커맨드 (페이즈 7) — 무인자는 기존 포그라운드 실행.
#[derive(Subcommand)]
enum DaemonCmd {
    /// OS 서비스로 등록 (linux=systemd 사용자 유닛, macOS=launchd, windows=SCM 서비스)
    Install {
        /// 이 서비스가 쓸 설정 파일 절대 경로 (미지정 시 OS 기본 경로) — 다중 프로필용
        #[arg(long)]
        config: Option<String>,
    },
    /// OS 서비스 등록 해제
    Uninstall,
    /// (windows 전용) SCM이 호출하는 서비스 진입점 — 직접 실행하지 말 것
    #[command(hide = true)]
    ServiceRun {
        #[arg(long)]
        config: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // 윈도우 서비스 모드: 콘솔이 없다 — 로그는 설정 디렉터리 파일로, 설정 경로는
    // SCM launch args에서 (에디션 2024의 unsafe set_var 대신 프로세스 내 override).
    // 런타임 진입 전에 분기 — SCM dispatcher는 자기 스레드를 점유한다
    if let Cmd::Daemon {
        action: Some(DaemonCmd::ServiceRun { config }),
    } = &cli.cmd
    {
        #[cfg(windows)]
        {
            if let Some(c) = config {
                config::set_path_override(c.into());
            }
            init_service_file_tracing()?;
            return brv::service::service_run();
        }
        #[cfg(not(windows))]
        {
            let _ = config;
            anyhow::bail!("service-run은 윈도우 SCM 전용이다");
        }
    }

    // stdout은 MCP 프로토콜 전용일 수 있다 — 로그는 항상 stderr로
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    tokio::runtime::Runtime::new()?.block_on(async_main(cli.cmd))
}

/// 서비스 프로세스의 로그 초기화 — stderr가 갈 곳이 없어 설정 디렉터리의 파일로.
#[cfg(windows)]
fn init_service_file_tracing() -> anyhow::Result<()> {
    let dir = config::config_path()?
        .parent()
        .expect("config path has parent")
        .to_path_buf();
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("daemon-service.log"))?;
    tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    Ok(())
}

async fn async_main(cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Init {
            server,
            enroll,
            admin_key,
            agent,
            channel,
            description,
            rotate,
            no_mcp,
        } => match enroll {
            Some(code) => enroll_init(server, code, channel, no_mcp).await,
            None => {
                // clap의 required_unless_present가 보장 — 여기 도달하면 전부 Some
                init(
                    server,
                    admin_key.context("clap invariant: admin_key")?,
                    agent.context("clap invariant: agent")?,
                    channel.context("clap invariant: channel")?,
                    description,
                    rotate,
                )
                .await
            }
        },
        Cmd::Status => status().await,
        Cmd::Send {
            to,
            payload,
            expects_ack,
        } => send(to, payload, expects_ack).await,
        Cmd::Listen => listen().await,
        Cmd::Mcp => mcp().await,
        Cmd::Daemon { action } => match action {
            None => {
                let cfg = config::load()?;
                let token = config::load_token(&cfg)?;
                brv::daemon::run(cfg, token).await
            }
            Some(DaemonCmd::Install { config }) => brv::service::install(config.as_deref()),
            Some(DaemonCmd::Uninstall) => brv::service::uninstall(),
            // main()이 런타임 진입 전에 처리한다
            Some(DaemonCmd::ServiceRun { .. }) => unreachable!("service-run은 main에서 분기"),
        },
    }
}

fn options_from_config() -> anyhow::Result<(BrvConfig, ClientOptions)> {
    let cfg = config::load()?;
    let token = config::load_token(&cfg)?;
    let mut opts = ClientOptions::new(&cfg.server, &cfg.channel, &cfg.agent, token);
    opts.description = cfg.description.clone();
    Ok((cfg, opts))
}

fn connect_from_config() -> anyhow::Result<(BrvConfig, Client)> {
    let (cfg, opts) = options_from_config()?;
    Ok((cfg, Client::connect(opts)))
}

fn token_store_note(stored: &config::TokenStore) -> String {
    match stored {
        config::TokenStore::Keyring => "토큰은 OS 키체인".to_owned(),
        config::TokenStore::File(p) => format!("키체인 없음 — 토큰 파일 {p:?} (600 권한)"),
    }
}

/// `brv init --enroll <코드>` — 대시보드 발급 코드 하나로 연결 (페이즈 10, PROTOCOL 10.1).
async fn enroll_init(
    server: String,
    code: String,
    channel: Option<String>,
    no_mcp: bool,
) -> anyhow::Result<()> {
    let enrolled = brv::enroll::exchange(&server, code.trim(), channel.as_deref()).await?;
    let stored = config::store_token(&enrolled.cfg, &enrolled.token)?;
    let path = config::store(&enrolled.cfg)?;
    println!(
        "연결 완료 — 조직 {org} / 에이전트 {agent} / 채널 {ch}",
        org = enrolled.org,
        agent = enrolled.cfg.agent,
        ch = enrolled.cfg.channel,
    );
    if enrolled.channels.len() > 1 {
        println!(
            "  부여된 다른 채널: {} (전환은 설정의 channel 값 또는 --channel)",
            enrolled
                .channels
                .iter()
                .filter(|c| **c != enrolled.cfg.channel)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!("  설정: {path:?} / {}", token_store_note(&stored));
    if no_mcp {
        println!("에이전트에서 쓰려면: claude mcp add brevduva -- brv mcp");
    } else {
        register_mcp();
    }
    Ok(())
}

/// Claude Code MCP 자동 등록 (기본 켬, --no-mcp로 생략) — 실패는 온보딩 실패가 아니라 안내.
/// --scope user: 기본 스코프(local)는 실행한 디렉터리에 묶여 온보딩 목적에 안 맞는다.
fn register_mcp() {
    match std::process::Command::new("claude")
        .args([
            "mcp", "add", "--scope", "user", "brevduva", "--", "brv", "mcp",
        ])
        .output()
    {
        Ok(out) if out.status.success() => {
            println!("Claude Code MCP 등록 완료 — 에이전트가 바로 채널을 쓸 수 있습니다");
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            if err.contains("already exists") {
                println!("Claude Code MCP는 이미 등록되어 있습니다");
            } else {
                println!(
                    "MCP 자동 등록 실패 — 수동 등록: claude mcp add brevduva -- brv mcp\n  ({})",
                    err.trim()
                );
            }
        }
        Err(_) => {
            println!(
                "claude CLI가 없어 MCP 등록을 건너뜀 — 에이전트에서 쓰려면: claude mcp add brevduva -- brv mcp"
            );
        }
    }
}

async fn init(
    server: String,
    admin_key: String,
    agent: String,
    channel: String,
    description: String,
    rotate: bool,
) -> anyhow::Result<()> {
    let http = reqwest::Client::new();
    let base = server.trim_end_matches('/').to_owned();

    // 1) 에이전트 등록 (409 + --rotate → 토큰 회전)
    let created = http
        .post(format!("{base}/v1/agents"))
        .bearer_auth(&admin_key)
        .json(&serde_json::json!({ "name": agent, "description": description }))
        .send()
        .await
        .context("server unreachable")?;
    let token = match created.status().as_u16() {
        201 => created.json::<serde_json::Value>().await?["token"]
            .as_str()
            .context("no token in response")?
            .to_owned(),
        409 if rotate => {
            let rotated = http
                .delete(format!("{base}/v1/agents/{agent}/token"))
                .bearer_auth(&admin_key)
                .send()
                .await?
                .error_for_status()
                .context("token rotate failed")?;
            rotated.json::<serde_json::Value>().await?["token"]
                .as_str()
                .context("no token in response")?
                .to_owned()
        }
        409 => anyhow::bail!(
            "agent {agent:?} already exists — rerun with --rotate to rotate its token \
             (this disconnects any session using the old token)"
        ),
        _ => anyhow::bail!("agent registration failed: {}", created.text().await?),
    };

    // 2) 채널 생성 (이미 있으면 통과) + grant
    let ch = http
        .post(format!("{base}/v1/channels"))
        .bearer_auth(&admin_key)
        .json(&serde_json::json!({ "name": channel }))
        .send()
        .await?;
    if !ch.status().is_success() && ch.status().as_u16() != 409 {
        anyhow::bail!("channel creation failed: {}", ch.text().await?);
    }
    http.post(format!("{base}/v1/channels/{channel}/grants"))
        .bearer_auth(&admin_key)
        .json(&serde_json::json!({ "agent": agent }))
        .send()
        .await?
        .error_for_status()
        .context("grant failed")?;

    // 3) 로컬 저장 — 토큰은 키체인, 설정은 파일 (클라이언트에 비밀 없음 원칙).
    // 기존 설정이 있으면 [wake] 등 운영 설정을 보존한다 — 재init·토큰 회전이 깨우기를 지우면 안 됨
    let wake = config::load().ok().and_then(|existing| existing.wake);
    let cfg = BrvConfig {
        server: base,
        channel,
        agent,
        description,
        wake,
    };
    let stored = config::store_token(&cfg, &token)?;
    let path = config::store(&cfg)?;

    println!(
        "초기화 완료 — 설정: {path:?} ({})",
        token_store_note(&stored)
    );
    println!();
    println!("Claude Code에 연결하려면:");
    println!("  claude mcp add brevduva -- brv mcp");
    println!();
    println!("수동 확인: brv status / brv listen / brv send --to <agent> --payload \"...\"");
    Ok(())
}

async fn status() -> anyhow::Result<()> {
    let cfg = config::load()?;
    println!(
        "설정: 서버 {} / 채널 {} / 에이전트 {}",
        cfg.server, cfg.channel, cfg.agent
    );
    let health = reqwest::Client::new()
        .get(format!("{}/healthz", cfg.server.trim_end_matches('/')))
        .send()
        .await;
    match health {
        Ok(resp) if resp.status().is_success() => println!("서버: OK"),
        Ok(resp) => println!("서버: 응답 이상 ({})", resp.status()),
        Err(e) => {
            println!("서버: 연결 불가 — {e}");
            return Ok(());
        }
    }
    let (_, client) = connect_from_config()?;
    match client.presence(Duration::from_secs(10)).await {
        Ok(entries) => {
            println!("채널 프레즌스:");
            for e in entries {
                println!("  {:10} {:?}", e.agent.as_str(), e.state);
            }
        }
        Err(e) => println!("프레즌스 조회 실패: {e}"),
    }
    Ok(())
}

async fn send(to: String, payload: String, expects_ack: bool) -> anyhow::Result<()> {
    let (_, client) = connect_from_config()?;
    let mut spec = PublishSpec::message(
        if to == "broadcast" || to.contains(':') {
            to
        } else {
            format!("agent:{to}")
        },
        payload,
    );
    if expects_ack {
        spec.expects = Some(brevduva_protocol::Expects::Ack);
    }
    match tokio::time::timeout(Duration::from_secs(10), client.publish(spec)).await {
        Ok(Ok(id)) => println!("sent {id}"),
        Ok(Err(e)) => anyhow::bail!("rejected: {} — {}", e.code, e.message),
        Err(_) => anyhow::bail!("unconfirmed after 10s — will republish on reconnect (13.3)"),
    }
    Ok(())
}

async fn listen() -> anyhow::Result<()> {
    let (cfg, client) = connect_from_config()?;
    eprintln!(
        "listening as {}@{} — Ctrl+C to stop",
        cfg.agent, cfg.channel
    );
    loop {
        if let Some(env) = client.recv(RecvFilter::Any, Duration::from_secs(60)).await {
            println!("{}", serde_json::to_string(&env)?);
        }
    }
}

async fn mcp() -> anyhow::Result<()> {
    // lazy-JOIN: 여기서 접속하지 않는다 — 첫 도구 호출 때 McpServer가 접속 (플랩 방지)
    let (cfg, opts) = options_from_config()?;
    tracing::info!(agent = %cfg.agent, channel = %cfg.channel, "brv mcp server on stdio");
    brv::mcp::run_stdio(opts).await
}
