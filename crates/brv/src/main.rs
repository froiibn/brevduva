// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! brv — Brevduva 리시버 CLI.
//!
//! `init`(셋업 일괄) · `binding`(다중 바인딩 관리) · `status` · `send` · `listen`(리시버 관찰) ·
//! `mcp`(러너 → 리시버 stdio 브리지) · `daemon`(상주 수신+깨우기) · `wake` · `hook`.
//!
//! **다중 바인딩 (페이즈 27)**: 설정은 여러 (에이전트, 채널) 바인딩을 담고, 데몬은 전부
//! 동시 수신한다. 단일 대상 명령(send·status·channels·wake test)은 바인딩이
//! 하나면 그것, 여럿이면 `--binding {agent}@{channel}` 명시를 요구한다 — 조용한 오발신 방지.
//! 세션(`mcp`)은 바인딩을 고르지 않는다 — 리시버에 붙어 `become`으로 정한다(RECEIVER_DESIGN P3).

use std::time::Duration;

use anyhow::Context as _;
use brv::config::{self, Binding, BrvConfig};
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
    /// Stream the current MCP's events into a host-owned Monitor
    SessionStream {
        #[arg(long)]
        address: String,
        #[arg(long)]
        ticket: String,
    },
    /// Connect an agent — with a one-time code (--enroll, recommended) or an admin
    /// API key. Bindings are **added** to an existing config (same agent@channel updates it)
    Init {
        /// Server base URL (e.g. https://api.brevduva.dev)
        #[arg(long)]
        server: String,
        /// One-time enroll code (issued in the dashboard's "Connect a machine") — no admin key needed
        #[arg(long)]
        enroll: Option<String>,
        /// Admin API key (BREVDUVA_ADMIN_KEY) — not needed with --enroll
        #[arg(
            long,
            env = "BREVDUVA_ADMIN_KEY",
            required_unless_present = "enroll",
            conflicts_with = "enroll"
        )]
        admin_key: Option<String>,
        /// Agent name for this machine (e.g. backend) — with enroll, the code decides
        #[arg(long, required_unless_present = "enroll", conflicts_with = "enroll")]
        agent: Option<String>,
        /// Channel (project) name — with enroll, picks among granted channels (defaults to the first)
        #[arg(long, required_unless_present = "enroll")]
        channel: Option<String>,
        /// Capability-declaration description — how peer agents decide to route to you (enroll uses the issued value)
        #[arg(long, default_value = "")]
        description: String,
        /// If the agent already exists, rotate its token and reuse it
        #[arg(long, conflicts_with = "enroll")]
        rotate: bool,
        /// Skip registering the MCP server in the agent runners detected on this machine
        #[arg(long)]
        no_mcp: bool,
        /// Runner to wake with when setting up unattended receiving (codex, claude, …) — skips the question when several are installed
        #[arg(long)]
        runner: Option<String>,
        /// Set up unattended receiving without asking (wake runner, one test wake, OS service)
        #[arg(long, conflicts_with = "attended_only")]
        unattended: bool,
        /// Attended use only: register the receiver service and leave unattended wake off
        #[arg(long)]
        attended_only: bool,
    },
    /// Manage bindings (agent × channel) — list, add, remove
    Binding {
        #[command(subcommand)]
        action: BindingCmd,
    },
    /// Check config, server, and channel status
    Status {
        /// Binding whose channel presence to query ({agent}@{channel}, or a unique agent name)
        #[arg(long)]
        binding: Option<String>,
    },
    /// List channels this agent may join (by token, PROTOCOL 10.2)
    Channels {
        /// Binding to query — required when multiple bindings exist
        #[arg(long)]
        binding: Option<String>,
    },
    /// Publish one message (for manual testing)
    Send {
        #[arg(long)]
        to: String,
        #[arg(long)]
        payload: String,
        /// Request ack collection on a broadcast (chapter 11)
        #[arg(long)]
        expects_ack: bool,
        /// Send as a reply to this message id (kind=reply + correlation — resolves the sender's wait_for_reply)
        #[arg(long)]
        reply_to: Option<String>,
        /// Sending binding — required when multiple bindings exist
        #[arg(long)]
        binding: Option<String>,
    },
    /// Watch what this machine's receiver does with incoming messages — where each went (a session, manual receive, an unattended wake, deferred). Takes nothing; Ctrl+C to stop
    Listen {
        /// Only this binding (org/agent@channel or agent@channel)
        #[arg(long)]
        binding: Option<String>,
        /// Print the raw event lines (JSON)
        #[arg(long)]
        json: bool,
    },
    /// Codex Desktop task helpers the receiver runs as the logged-on user
    Desktop {
        #[command(subcommand)]
        action: DesktopCmd,
    },
    /// Local MCP bridge (stdio) from agent runners to this machine's receiver — or `brv mcp register` to add it to the runners on this machine
    Mcp {
        /// Removed (2026-09-09): sessions take an identity with `become` — kept only to explain stale registrations
        #[arg(long, hide = true)]
        binding: Option<String>,
        /// Absolute path to the config file (default: BREVDUVA_CONFIG env, then the OS path) — runner registrations pin it
        #[arg(long)]
        config: Option<String>,
        /// Runner that launches this process — set by `brv mcp register`, never inferred (2026-09-05)
        #[arg(long, hide = true)]
        host: Option<String>,
        #[command(subcommand)]
        action: Option<McpCmd>,
    },
    /// Resident receiver — holds the channel slot for every binding and delivers to local sessions; wakes a session per message when [wake] is configured
    Daemon {
        // 서비스가 아닌 실행 표면(작업 스케줄러 로그온 작업 등)에서 프로필을 고정하는 통로
        // — 2026-09-01, 윈도우 PIN 전용 사용자의 무암호 상주 경로에서 필요 실측
        /// Absolute path to the config file (default: BREVDUVA_CONFIG env, then the OS path)
        #[arg(long)]
        config: Option<String>,
        #[command(subcommand)]
        action: Option<DaemonCmd>,
    },
    /// Claude Code hook integration — checks pending messages across bindings at turn end
    Hook {
        #[command(subcommand)]
        action: HookCmd,
    },
    /// Unattended wake settings — what the daemon runs on message arrival, and with what allowance
    Wake {
        #[command(subcommand)]
        action: WakeCmd,
    },
}

#[derive(Subcommand)]
enum DesktopCmd {
    /// Check the existing Desktop owner without sending input or joining
    Check {
        #[arg(long)]
        thread: String,
    },
    /// Receiver-internal: start one turn in a Desktop task for one delivery (runs as the logged-on user)
    #[command(hide = true)]
    Submit {
        #[arg(long)]
        thread: String,
        #[arg(long)]
        message_id: String,
        #[arg(long)]
        receipt: String,
        #[arg(long, default_value_t = 45)]
        busy_wait_secs: u64,
    },
}

#[derive(Subcommand)]
enum BindingCmd {
    /// List configured bindings (+token presence, wake settings)
    List,
    /// Add a binding with an existing token — same agent on another channel (grant is checked server-side)
    Add {
        /// Agent name (its token must already be on this machine — enroll first if not)
        #[arg(long)]
        agent: String,
        /// Channel to join (after granting it in the dashboard)
        #[arg(long)]
        channel: String,
        /// Capability-declaration description
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Remove a binding — the token stays (other bindings of the same agent may use it)
    Remove {
        /// {agent}@{channel}
        selector: String,
    },
}

/// `brv wake` — [wake]는 설정 파일에 저장되어 재부팅·데몬 재시작·재init 후에도 유지된다.
/// 권한은 로컬 신뢰 정책: 이 머신의 파일로만 정해지고 서버·원격 메시지는 바꿀 수 없다.
/// 페이즈 27 분리: 실행기·권한·타임아웃은 전역, 작업 디렉터리·정책은 바인딩별.
#[derive(Subcommand)]
enum WakeCmd {
    /// Create or update wake settings — changes only the values you pass (idempotent)
    Set {
        /// Unattended-session allowance: respond (reply only, default)|edit (+file edits)|full (+shell).
        /// With --binding, overrides args for that binding only; otherwise global
        #[arg(long)]
        allow: Option<String>,
        /// Runner profile to wake with (codex, claude, gemini, …) — finds its executable on this machine and
        /// sets the one-shot arguments for it. `brv status` lists detected runners. With --binding, that binding only
        #[arg(long)]
        runner: Option<String>,
        /// Wake executable path — for a runner not in the profile table, or to pin a specific binary.
        /// If omitted, the runner is detected (exactly one must be installed, else pass --runner)
        #[arg(long)]
        command: Option<String>,
        /// Working directory for woken sessions — **per binding** (new single binding defaults to the current directory)
        #[arg(long)]
        dir: Option<String>,
        /// Max run time for a woken session in seconds — always global (machine policy)
        #[arg(long)]
        timeout: Option<u64>,
        /// Target binding for per-binding values (--dir, and --command/--allow as overrides)
        #[arg(long)]
        binding: Option<String>,
    },
    /// Show current wake settings and the effective command line
    Show,
    /// Run one real wake with a harmless prompt — verifies the command path and environment
    Test {
        /// Which binding's wake_dir to run in — required when multiple bindings exist
        #[arg(long)]
        binding: Option<String>,
    },
}

/// `brv mcp register` (2026-09-04, 온보딩 재설계 1): 이 머신에서 탐지된 러너 **전부**에 로컬 MCP
/// 서버를 등록한다 — 어느 러너를 열어도 Brevduva 도구가 있게(유인용). 깨우기 러너 선택과는
/// 별개다(그건 바인딩당 하나, `brv wake set --runner`).
#[derive(Subcommand)]
enum McpCmd {
    /// Register the local `brv mcp` server in every agent runner detected on this machine
    Register {
        /// Only this runner (codex, claude, gemini, …) — default: all detected
        #[arg(long)]
        runner: Option<String>,
        /// Print the registration commands/snippets without running anything
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum HookCmd {
    /// Register the Stop hook in ~/.claude/settings.json (idempotent)
    Install,
    /// Stop-hook entry point — invoked by Claude Code (never run directly)
    #[command(hide = true)]
    Stop,
}

/// `brv daemon`의 서비스 등록 서브커맨드 (페이즈 7) — 무인자는 기존 포그라운드 실행.
#[derive(Subcommand)]
enum DaemonCmd {
    /// Register as an OS service (linux=systemd user unit, macOS=launchd, windows=SCM service as LocalSystem — run once from an administrator terminal; wakes run in your logged-on session)
    Install {
        /// Absolute path to the config file this service uses (default: OS path) — for multiple profiles
        #[arg(long)]
        config: Option<String>,
    },
    /// Unregister the OS service
    Uninstall,
    /// Restart the registered OS service (config/token changes apply on restart)
    Restart,
    /// Pause the daemon for a while — it leaves the channel and messages queue server-side (for when an interactive session handles the channel itself)
    Pause {
        /// How long, e.g. 30m, 2h (default 1h)
        #[arg(long = "for", default_value = "1h")]
        duration: String,
    },
    /// End a pause early — the daemon re-checks wake and rejoins
    Resume,
    /// (windows only) service entry point invoked by SCM — never run directly
    #[command(hide = true)]
    ServiceRun {
        #[arg(long)]
        config: Option<String>,
        /// Wake sessions run as this logged-on user (the installer)
        #[arg(long)]
        wake_user: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // 윈도우 서비스 모드: 콘솔이 없다 — 로그는 설정 디렉터리 파일로, 설정 경로는
    // SCM launch args에서 (에디션 2024의 unsafe set_var 대신 프로세스 내 override).
    // 런타임 진입 전에 분기 — SCM dispatcher는 자기 스레드를 점유한다
    if let Cmd::Daemon {
        action: Some(DaemonCmd::ServiceRun { config, wake_user }),
        ..
    } = &cli.cmd
    {
        #[cfg(windows)]
        {
            if let Some(c) = config {
                config::set_path_override(c.into());
            }
            init_service_file_tracing()?;
            return brv::service::service_run(wake_user.clone());
        }
        #[cfg(not(windows))]
        {
            let _ = (config, wake_user);
            anyhow::bail!("service-run is Windows SCM only");
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

/// 무인 세션(데몬이 깨운 세션 — `BREVDUVA_BINDING`을 물려받는다) 안에서 이 머신의 로컬 정책을
/// 바꾸는 명령 (2026-09-03, 실사고: 에이전트가 `brv wake set --policy never`로 자기 깨우기를
/// 껐다). 원격 메시지가 로컬 정책을 바꾸는 경로를 막는다 — 페이즈 21 원칙의 구멍 봉합.
/// 보안 경계가 아니라 난간이다: full 권한 세션은 파일을 직접 고칠 수 있으므로 진짜 경계는
/// 허용 수준(respond/edit)이다.
fn changes_local_policy(cmd: &Cmd) -> bool {
    matches!(
        cmd,
        Cmd::Init { .. }
            | Cmd::Binding {
                action: BindingCmd::Add { .. } | BindingCmd::Remove { .. },
            }
            | Cmd::Wake {
                action: WakeCmd::Set { .. },
            }
            | Cmd::Mcp {
                action: Some(_),
                ..
            }
            | Cmd::Daemon {
                action: Some(
                    DaemonCmd::Install { .. }
                        | DaemonCmd::Uninstall
                        | DaemonCmd::Restart
                        | DaemonCmd::Pause { .. }
                        | DaemonCmd::Resume
                ),
                ..
            }
            | Cmd::Hook {
                action: HookCmd::Install,
            }
    )
}

async fn async_main(cmd: Cmd) -> anyhow::Result<()> {
    if std::env::var_os("BREVDUVA_BINDING").is_some() && changes_local_policy(&cmd) {
        anyhow::bail!(
            "refused: this command changes the receiver's local policy and is not allowed from an unattended (daemon-woken) session — tell the sender the machine owner must run it"
        );
    }
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
            runner,
            unattended,
            attended_only,
        } => match enroll {
            Some(code) => {
                enroll_init(
                    server,
                    code,
                    channel,
                    no_mcp,
                    runner.as_deref(),
                    unattended,
                    attended_only,
                )
                .await
            }
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
        Cmd::Binding { action } => match action {
            BindingCmd::List => binding_list(),
            BindingCmd::Add {
                agent,
                channel,
                description,
            } => binding_add(agent, channel, description).await,
            BindingCmd::Remove { selector } => binding_remove(&selector),
        },
        Cmd::Status { binding } => status(binding.as_deref()).await,
        Cmd::Channels { binding } => channels(binding.as_deref()).await,
        Cmd::Send {
            to,
            payload,
            expects_ack,
            reply_to,
            binding,
        } => send(to, payload, expects_ack, reply_to, binding.as_deref()).await,
        Cmd::Listen { binding, json } => listen(binding.as_deref(), json).await,
        Cmd::SessionStream { address, ticket } => {
            brv::session_delivery::stream(&address, &ticket).await
        }
        Cmd::Desktop { action } => match action {
            DesktopCmd::Check { thread } => brv::desktop::check(&thread).await,
            DesktopCmd::Submit {
                thread,
                message_id,
                receipt,
                busy_wait_secs,
            } => {
                brv::desktop::submit(
                    &thread,
                    &message_id,
                    &receipt,
                    Duration::from_secs(busy_wait_secs),
                )
                .await
            }
        },
        Cmd::Mcp {
            binding,
            config,
            host,
            action,
        } => {
            if let Some(c) = config {
                let p = std::path::PathBuf::from(&c);
                anyhow::ensure!(p.is_absolute(), "--config must be an absolute path: {c}");
                config::set_path_override(p);
            }
            match action {
                Some(McpCmd::Register { runner, dry_run }) => {
                    mcp_register(runner.as_deref(), dry_run)
                }
                None => mcp(binding.as_deref(), host).await,
            }
        }
        Cmd::Daemon { config, action } => match action {
            None => {
                // 포그라운드 프로필 고정 (2026-09-01) — 서비스 모드의 PATH_OVERRIDE와 같은 통로
                if let Some(c) = config {
                    config::set_path_override(c.into());
                }
                let cfg = config::load()?;
                let tokens = config::load_tokens(&cfg)?;
                // 토큰 거부 시 저장소 재읽기 + 기동 시 깨우기 사전 점검 (2026-09-02)
                let reload_cfg = cfg.clone();
                brv::daemon::run_with_options(
                    cfg,
                    tokens,
                    brv::daemon::DaemonOptions {
                        token_reload: Some(std::sync::Arc::new(move |b: &Binding| {
                            config::load_token(&reload_cfg, b).ok()
                        })),
                        preflight: true,
                        ..Default::default()
                    },
                )
                .await
            }
            Some(DaemonCmd::Install { config }) => brv::service::install(config.as_deref()),
            Some(DaemonCmd::Uninstall) => brv::service::uninstall(),
            Some(DaemonCmd::Restart) => restart_daemon(true),
            Some(DaemonCmd::Pause { duration }) => pause_daemon(&duration),
            Some(DaemonCmd::Resume) => resume_daemon(),
            // main()이 런타임 진입 전에 처리한다
            Some(DaemonCmd::ServiceRun { .. }) => unreachable!("service-run branches in main"),
        },
        Cmd::Wake { action } => match action {
            WakeCmd::Set {
                allow,
                runner,
                command,
                dir,
                timeout,
                binding,
            } => wake_set(
                allow,
                runner.as_deref(),
                command,
                dir,
                timeout,
                binding.as_deref(),
            ),
            WakeCmd::Show => wake_show(),
            WakeCmd::Test { binding } => wake_test(binding.as_deref()).await,
        },
        Cmd::Hook { action } => match action {
            HookCmd::Install => {
                println!("{}", brv::hook::install()?);
                Ok(())
            }
            HookCmd::Stop => {
                // 훅은 조용해야 한다 — 설정 부재·서버 장애는 침묵 종료 (세션을 방해하지 않음)
                let Ok(cfg) = config::load() else {
                    return Ok(());
                };
                // 전 바인딩 합산 (페이즈 27) — 토큰 없는 바인딩은 조용히 건너뛴다
                let targets: Vec<brv::hook::HookTarget> = cfg
                    .bindings
                    .iter()
                    .filter_map(|b| {
                        config::load_token(&cfg, b)
                            .ok()
                            .map(|token| brv::hook::HookTarget {
                                agent: b.agent.clone(),
                                channel: b.channel.clone(),
                                token,
                            })
                    })
                    .collect();
                if targets.is_empty() {
                    return Ok(());
                }
                let mut stdin_json = String::new();
                use tokio::io::AsyncReadExt as _;
                let _ = tokio::io::stdin().read_to_string(&mut stdin_json).await;
                if let Some(block) = brv::hook::stop(&cfg.server, &targets, &stdin_json).await {
                    println!("{block}");
                }
                Ok(())
            }
        },
    }
}

/// PATH에서 실행 파일 탐색 — 설정에는 항상 절대 경로로 저장하기 위함.
/// 2026-08-29 실사고의 교훈: 서비스(systemd 등) 환경의 PATH에는 사용자 설치 경로가 없어
/// 상대 이름 "claude"가 안 풀렸다. 설정 시점에 절대 경로로 못 박으면 재발하지 않는다.
fn find_in_path(name: &str) -> Option<std::path::PathBuf> {
    brv::runners::find_in_path(name)
}

/// 실행 파일 경로 해석 — 결과는 항상 절대 경로 (2026-08-29 실사고: 서비스 환경 PATH에는
/// 사용자 설치 경로가 없다). 전역 [wake]와 바인딩별 러너 오버라이드가 공유한다.
fn resolve_command(c: String) -> anyhow::Result<String> {
    if std::path::Path::new(&c).is_absolute() {
        return Ok(c);
    }
    // 경로 구분자가 있으면 현재 디렉터리 기준, 맨 이름이면 PATH에서
    if c.contains('/') || c.contains('\\') {
        return Ok(std::fs::canonicalize(&c)
            .with_context(|| format!("cannot resolve {c:?} from the current directory"))?
            .to_string_lossy()
            .into_owned());
    }
    Ok(find_in_path(&c)
        .with_context(|| format!("{c:?} not found in PATH — pass an absolute --command"))?
        .to_string_lossy()
        .into_owned())
}

/// `brv wake set` — 전역([wake]: 실행기·권한·타임아웃)과 바인딩별(dir, 그리고
/// --binding과 결합된 --command/--allow = 러너 오버라이드)을 한 명령으로.
/// 지정한 값만 갱신하고 나머지는 유지(멱등). 설정 파일이 단일 진실이라 재부팅·데몬
/// 재시작·재init(보존은 upsert가 담당)에도 계속 유지된다.
fn wake_set(
    allow: Option<String>,
    runner: Option<&str>,
    command: Option<String>,
    dir: Option<String>,
    timeout: Option<u64>,
    binding_sel: Option<&str>,
) -> anyhow::Result<()> {
    let mut cfg = config::load()?; // 연결 설정(init) 위에 얹는다 — 미init이면 여기서 안내됨
    // --binding이 있으면 --runner/--command/--allow는 그 바인딩의 오버라이드 (2026-09-01 러너 혼용)
    let binding_scoped = binding_sel.is_some();
    if let Some(level) = &allow {
        anyhow::ensure!(
            matches!(level.as_str(), "respond" | "edit" | "full"),
            "unknown --allow {level:?} — one of: respond, edit, full"
        );
    }
    let existing = cfg.wake.take();
    // 지금 이 스코프가 쓰는 실행 파일 — 바인딩 오버라이드 → 전역
    let current_cmd = if binding_scoped {
        cfg.select(binding_sel)?
            .wake_command
            .clone()
            .or_else(|| existing.as_ref().map(|w| w.command.clone()))
    } else {
        existing.as_ref().map(|w| w.command.clone())
    };

    // ---- 러너 결정 (2026-09-04 프로필) ----
    // --runner: 표에서 찾아 이 머신의 실행 파일을 탐지 (--command가 있으면 그 경로를 그 프로필로)
    // --command만: 경로를 쓰고 파일 이름으로 프로필을 역추정 (표에 없으면 custom)
    // 둘 다 없음: 기존 설정 유지. 기존도 없으면 탐지 결과가 **하나일 때만** 자동 — 여럿이면 묻지
    // 않고 --runner를 요구한다 (조용히 고르면 엉뚱한 러너가 깨어난다)
    let (new_cmd, spec): (Option<String>, Option<&'static brv::runners::RunnerSpec>) = match (
        runner, &command,
    ) {
        (Some(id), c) => {
            let spec = brv::runners::spec(id).with_context(|| {
                format!(
                    "unknown runner {id:?} — known: {}",
                    brv::runners::RUNNERS
                        .iter()
                        .map(|r| r.id)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            let path = match c {
                Some(c) => resolve_command(c.clone())?,
                None => {
                    let det = brv::runners::detect(spec).with_context(|| {
                            format!(
                                "{} not found on this machine — install it, or pass --command <absolute path>",
                                spec.display
                            )
                        })?;
                    println!(
                        "runner {}: {} ({})",
                        spec.id,
                        det.path.display(),
                        det.version
                    );
                    det.path.to_string_lossy().into_owned()
                }
            };
            (Some(path), Some(spec))
        }
        (None, Some(c)) => {
            let path = resolve_command(c.clone())?;
            (Some(path.clone()), brv::runners::spec_for_command(&path))
        }
        (None, None) => match &current_cmd {
            Some(c) => (None, brv::runners::spec_for_command(c)),
            None => {
                let found: Vec<_> = brv::runners::detect_all()
                    .into_iter()
                    .filter(|d| d.spec.wake.is_some())
                    .collect();
                match found.as_slice() {
                    [] => anyhow::bail!(
                        "no agent runner found on this machine — install one (Codex, Claude Code, Gemini CLI, …) \
                             or pass --command <absolute path>. `brv status` shows what was looked for"
                    ),
                    [one] => {
                        println!(
                            "runner {}: {} ({}) — the only one found",
                            one.spec.id,
                            one.path.display(),
                            one.version
                        );
                        (
                            Some(one.path.to_string_lossy().into_owned()),
                            Some(one.spec),
                        )
                    }
                    many => anyhow::bail!(
                        "several runners found — pick one with --runner: {}",
                        many.iter()
                            .map(|d| format!("{} ({})", d.spec.id, d.version))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                }
            }
        },
    };
    if let Some(s) = spec
        && !s.wake_measured
        && new_cmd.is_some()
    {
        eprintln!(
            "warning: the {} wake profile is documented but not yet measured by brv — run `brv wake test` and check the reply path",
            s.display
        );
    }
    // 권한 수준 → 이 러너의 인자. 표에 없는 러너(custom)는 프리셋이 없으니 손으로 적어야 한다
    let args_for = |level: &str| -> anyhow::Result<Vec<String>> {
        let s = spec.context(
            "--allow needs a runner from the profile table — for a custom command edit `args`/`wake_args` in the config file by hand",
        )?;
        brv::runners::wake_args(s, level)
            .with_context(|| format!("{} has no arguments for level {level:?}", s.display))
    };
    // 러너를 바꾸면 옛 러너의 인자는 무의미하다 — 기존 수준을 새 러너로 옮겨 다시 만든다
    let level_of_existing = |cmd: &str, args: &[String]| -> &'static str {
        brv::runners::spec_for_command(cmd)
            .and_then(|s| brv::runners::level_of(s, args))
            .unwrap_or("respond")
    };

    // ---- 전역부: 바인딩 스코프가 아닐 때의 러너/권한 + 항상 전역인 --timeout ----
    let timeout_s = timeout
        .or(existing.as_ref().map(|w| w.timeout_s))
        .unwrap_or(600);
    if binding_scoped {
        let mut w = existing
            .context("no [wake] configured — run `brv wake set` without --binding first")?;
        w.timeout_s = timeout_s;
        cfg.wake = Some(w);
    } else {
        let g_command = new_cmd
            .clone()
            .or_else(|| existing.as_ref().map(|w| w.command.clone()))
            .expect("a command was resolved or already configured");
        let g_args = match (&allow, &existing, new_cmd.is_some()) {
            (Some(level), _, _) => args_for(level)?,
            (None, Some(w), false) => w.args.clone(),
            (None, Some(w), true) => args_for(level_of_existing(&w.command, &w.args))?,
            (None, None, _) => args_for("respond")?,
        };
        cfg.wake = Some(config::WakeConfig {
            command: g_command,
            args: g_args,
            timeout_s,
        });
    }

    // ---- 바인딩부 (페이즈 27): dir + 스코프된 러너/권한 ----
    let needs_binding = binding_scoped || dir.is_some();
    // 신규 단일 바인딩에서 wake_dir 미설정이면 현재 디렉터리를 기본으로 —
    // "프로젝트 루트에서 설정한다"는 페이즈 21의 자연 동작 유지
    let default_dir = cfg.bindings.len() == 1 && cfg.bindings[0].wake_dir.is_none();
    if needs_binding || default_dir {
        let full = cfg.select(binding_sel)?.full_label();
        let dir = match &dir {
            Some(d) => Some(d.clone()),
            None if default_dir => Some(std::env::current_dir()?.to_string_lossy().into_owned()),
            None => None,
        };
        let target = cfg
            .bindings
            .iter_mut()
            .find(|b| b.full_label() == full)
            .expect("select returned an existing binding");
        if let Some(d) = dir {
            target.wake_dir = Some(d);
        }
        if binding_scoped {
            if let Some(c) = &new_cmd {
                let prev_level = match (&target.wake_command, &target.wake_args) {
                    (Some(pc), Some(pa)) => level_of_existing(pc, pa),
                    _ => "respond",
                };
                target.wake_command = Some(c.clone());
                if allow.is_none() {
                    target.wake_args = Some(args_for(prev_level)?);
                }
            }
            if let Some(level) = &allow {
                target.wake_args = Some(args_for(level)?);
            }
        }
    }

    let path = config::store(&cfg)?;
    println!("wake configured — saved to {path:?} (survives reboots, daemon restarts, re-init)");
    wake_show()?;
    restart_daemon(false)?;
    println!("\nnext: `brv wake test` to verify it actually spawns, then `brv daemon install`");
    Ok(())
}

/// `brv wake show` — 저장된 설정과 실효 명령줄. "지금 깨워지면 정확히 이렇게 실행된다".
fn wake_show() -> anyhow::Result<()> {
    let cfg = config::load()?;
    let Some(wake) = &cfg.wake else {
        println!("no [wake] configured — run `brv wake set --allow respond|edit|full`");
        return Ok(());
    };
    let describe = |cmd: &str, args: &[String]| -> (String, &'static str) {
        match brv::runners::spec_for_command(cmd) {
            Some(s) => (
                if s.wake_measured {
                    s.id.to_owned()
                } else {
                    format!("{} (profile not yet measured — run `brv wake test`)", s.id)
                },
                brv::runners::level_of(s, args).unwrap_or("custom (hand-edited args)"),
            ),
            None => (
                "custom (not in the profile table)".to_owned(),
                "custom (hand-edited args)",
            ),
        }
    };
    let (runner, level) = describe(&wake.command, &wake.args);
    println!("runner   : {runner} (global)");
    println!("allow    : {level} (global)");
    if let Some(tools) = config::wake_allowed_tools(&wake.args) {
        println!("tools    : {tools}");
    }
    println!("command  : {} (global)", wake.command);
    println!("timeout  : {}s", wake.timeout_s);
    if let Some(warning) = script_prompt_warning(&wake.command, &wake.args) {
        println!("warning  : {warning}");
    }
    println!("bindings :");
    for b in &cfg.bindings {
        let eff = brv::daemon::effective_wake(wake, b);
        if (b.wake_command.is_some() || b.wake_args.is_some())
            && let Some(warning) = script_prompt_warning(&eff.command, &eff.args)
        {
            println!("  warning: {} — {warning}", b.full_label());
        }
        let runner = if b.wake_command.is_some() || b.wake_args.is_some() {
            let (id, level) = describe(&eff.command, &eff.args);
            format!("runner {id} / allow {level} (override)")
        } else {
            "runner (global)".to_owned()
        };
        println!(
            "  {:34} dir {} — {runner}",
            b.full_label(),
            b.wake_dir.as_deref().unwrap_or("(unset — wake blocked)")
        );
    }
    Ok(())
}

/// 윈도우 `.cmd`·`.bat` 러너에 프롬프트를 인자로 넘기는 설정의 경고 (2026-09-13, U7): `cmd.exe` 감싸기가
/// 여러 줄 프롬프트를 첫 줄에서 자른다. 사전 점검·`wake test`의 한 줄 프롬프트는 통과하므로 여기서 말해야 한다.
fn script_prompt_warning(command: &str, args: &[String]) -> Option<String> {
    (cfg!(windows)
        && brv::daemon::is_script_runner(command)
        && !brv::daemon::prompt_via_stdin(args))
    .then(|| {
        format!(
            "{command} is a .cmd/.bat script and these args pass the prompt as an argument — on Windows cmd.exe cuts a multi-line prompt at the first line, so real wakes lose the message. Use a profile that reads the prompt from stdin (`brv wake set --runner codex` does) or point the command at the runner's native executable"
        )
    })
}

/// `brv wake test` — 실제 깨우기와 같은 스폰 경로로 1회 실행해 환경을 검증한다.
/// 2026-08-29 실사고(서비스 PATH에 claude 부재)를 설정 시점에 잡는 검사.
async fn wake_test(binding_sel: Option<&str>) -> anyhow::Result<()> {
    let cfg = config::load()?;
    let global = cfg
        .wake
        .clone()
        .context("no [wake] configured — run `brv wake set` first")?;
    let binding = cfg.select(binding_sel)?;
    let dir = binding.wake_dir.clone().with_context(|| {
        format!(
            "binding {} has no wake_dir — set with `brv wake set --dir <project> --binding {}`",
            binding.full_label(),
            binding.full_label()
        )
    })?;
    // 실제 깨우기와 같은 계산 — 바인딩 러너 오버라이드 포함. 검증은 짧게(상한 120초)
    let wake = brv::daemon::effective_wake(&global, binding);
    let capped = config::WakeConfig {
        timeout_s: wake.timeout_s.min(120),
        ..wake
    };
    let prompt = brv::daemon::WAKE_TEST_PROMPT;
    println!(
        "spawning wake session: {} (binding {}, dir {}, cap {}s)...",
        capped.command,
        binding.full_label(),
        dir,
        capped.timeout_s
    );
    let started = std::time::Instant::now();
    let child = brv::daemon::spawn_wake(
        &capped,
        &dir,
        &binding.full_label(),
        prompt,
        &brv::daemon::WakeSpawn::Direct,
        // 점검용 깨우기라 잠글 작업이 없다 — 깨우기 창 식별자도 없다.
        None,
    )
    .await?;
    println!("spawn OK — waiting for the session to exit...");
    let log_hint = config::config_path()?
        .parent()
        .expect("config has parent")
        .join("wake.log");
    let status = match tokio::time::timeout(Duration::from_secs(capped.timeout_s), async {
        let mut child = child;
        child.wait().await
    })
    .await
    {
        Ok(res) => res.context("wake process wait")?,
        Err(_) => anyhow::bail!(
            "wake session did not finish within {}s — check {log_hint:?}",
            capped.timeout_s
        ),
    };
    anyhow::ensure!(
        status.success(),
        "wake session exited with {status} — check {log_hint:?}"
    );
    println!(
        "WAKE TEST OK ({:.1}s) — session output appended to {log_hint:?}",
        started.elapsed().as_secs_f32()
    );
    // 한 줄 점검 프롬프트로는 드러나지 않는 결함 (U7) — 여기서 알린다
    if let Some(warning) = script_prompt_warning(&capped.command, &capped.args) {
        println!("warning: {warning}");
    }
    Ok(())
}

/// `brv channels` — 선택 바인딩 에이전트의 grant 채널 목록 (이 머신에 바인딩된 채널 표시).
async fn channels(binding_sel: Option<&str>) -> anyhow::Result<()> {
    let cfg = config::load()?;
    let binding = cfg.select(binding_sel)?;
    let token = config::load_token(&cfg, binding)?;
    let (org, agent, list) = brv::client::discover_channels(&cfg.server, &token).await?;
    println!("agent {agent} @ org {org} — channels this token may join:");
    for ch in &list {
        // 같은 정체성(org까지 동일한 에이전트)의 바인딩만 * 표시
        let bound = cfg
            .bindings
            .iter()
            .any(|b| b.token_id() == binding.token_id() && b.channel == *ch);
        let marker = if bound { "* " } else { "  " };
        println!("{marker}{ch}");
    }
    if list.is_empty() {
        println!("  (none — grant channel access in the dashboard)");
    } else {
        println!(
            "(* = bound on this machine. add: brv binding add --agent {agent} --channel <ch>)"
        );
    }
    Ok(())
}

fn token_store_note(stored: &config::TokenStore) -> String {
    match stored {
        config::TokenStore::Keyring => "token in the OS keychain".to_owned(),
        config::TokenStore::File(p) => format!("no usable keychain — token file {p:?} (mode 600)"),
    }
}

/// 설정 파일을 병합용으로 연다 — 없으면 새 설정, 있으면 서버 일치 검증(설정 하나=서버 하나).
/// 파손된 기존 설정은 정직하게 실패한다 (덮어써서 다른 바인딩을 날리지 않는다).
fn open_config_for(server: &str) -> anyhow::Result<BrvConfig> {
    let path = config::config_path()?;
    if path.exists() {
        let existing = config::load()?;
        anyhow::ensure!(
            existing.server == server,
            "config already targets {} — one config, one server. Use a separate BREVDUVA_CONFIG profile for another server",
            existing.server
        );
        Ok(existing)
    } else {
        Ok(BrvConfig {
            server: server.to_owned(),
            wake: None,
            bindings: Vec::new(),
        })
    }
}

/// init/binding add 결과를 기존 설정에 병합 — 페이즈 27: 덮어쓰기가 아니라 바인딩 upsert.
fn merge_binding(server: &str, binding: Binding) -> anyhow::Result<(BrvConfig, bool)> {
    let mut cfg = open_config_for(server)?;
    let replaced = cfg.upsert_binding(binding);
    Ok((cfg, replaced))
}

/// `brv init --enroll <코드>` — 대시보드 발급 코드 하나로 연결 (페이즈 10, PROTOCOL 10.1).
/// 다중 에이전트 코드(2026-09-02)는 나열된 (에이전트, 채널) 쌍 전부를 한 번에 바인딩한다 —
/// 설정은 한 번 열어 전부 upsert하고 한 번 저장한다.
async fn enroll_init(
    server: String,
    code: String,
    channel: Option<String>,
    no_mcp: bool,
    runner: Option<&str>,
    unattended: bool,
    attended_only: bool,
) -> anyhow::Result<()> {
    let enrolled = brv::enroll::exchange(&server, code.trim(), channel.as_deref()).await?;
    let mut cfg = open_config_for(&enrolled.server)?;
    let mut stored = None;
    for ea in &enrolled.agents {
        let mut channels = Vec::with_capacity(ea.bindings.len());
        let mut replaced = false;
        for b in &ea.bindings {
            channels.push(b.channel.clone());
            replaced |= cfg.upsert_binding(b.clone());
        }
        // 토큰은 에이전트당 하나 — 첫 바인딩의 정체성 키(org/agent)로 저장
        let agent = &ea.bindings[0];
        stored = Some(config::store_token(&cfg.server, agent, &ea.token)?);
        println!(
            "connected — org {org} / agent {agent} / channel{s} {chs}{note}",
            org = enrolled.org,
            agent = agent.agent,
            s = if channels.len() > 1 { "s" } else { "" },
            chs = channels.join(", "),
            note = if replaced {
                " (existing binding updated — token rotated)"
            } else {
                ""
            },
        );
        let unbound: Vec<&str> = ea
            .channels
            .iter()
            .filter(|c| !channels.contains(c))
            .map(String::as_str)
            .collect();
        if !unbound.is_empty() {
            println!(
                "  also granted: {} (receive there too: brv binding add --agent {} --channel <ch>)",
                unbound.join(", "),
                agent.agent,
            );
        }
    }
    let stored = stored.context("enroll response listed no agents")?;
    let path = config::store(&cfg)?;
    println!("  config: {path:?} / {}", token_store_note(&stored));
    if no_mcp {
        println!("to register the MCP server in your agent runners later: brv mcp register");
    } else {
        register_mcp();
    }
    // 돌고 있는 데몬에 새 토큰·바인딩을 즉시 반영 (2026-09-02 맥북 실사고 — 재enroll 후 재기동
    // 없이는 데몬이 옛 토큰으로 죽어 있었다). 서비스 미등록이면 조용히 지나간다
    restart_daemon(false)?;
    // 무인 수신 통합 흐름 (2026-09-04, 온보딩 재설계 2): 종전엔 세 명령을 안내만 했다 —
    // 이제 질문 하나로 잇는다. 이미 무인 설정+서비스가 있는 머신(두 번째 에이전트)은 묻지 않는다
    let service = brv::service::registered();
    if cfg.wake.is_some() && service {
        println!(
            "unattended receiving is already set up on this machine — the daemon picked up the new binding"
        );
        return Ok(());
    }
    use std::io::IsTerminal as _;
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let go = if unattended {
        true
    } else if attended_only || !interactive {
        // 터미널이 아니면(스크립트·에이전트가 대신 실행) 묻지 않는다 — 플래그로 정한다
        false
    } else {
        println!();
        ask_yes_no(
            "Also receive while you're away — wake an agent session per message? [Y/n] ",
            true,
        )?
    };
    if go {
        return setup_unattended(&cfg, runner, interactive).await;
    }
    // 유인 전용도 리시버 서비스는 필요하다 (2026-09-11 번복, 16단계): 세션은 리시버에 붙어 보내고 받는다 —
    // `--attended-only`는 "서비스 없음"이 아니라 "서비스는 등록하되 무인 깨우기는 끔"이다. 깨우기 설정이 없는
    // 리시버는 세션이 바인딩을 쥘 때만 서버에 붙는다. 터미널 밖에서 플래그 없이 실행되면 관리자 승인 창을
    // 띄우지 않도록 등록하지 않고 안내만 한다.
    println!();
    if service {
        println!("attended use is ready — the receiver service picked up the new binding");
    } else if attended_only || interactive {
        match brv::service::install(None) {
            Ok(()) => println!(
                "attended use is ready — sessions attach to the receiver service (unattended wake stays off)"
            ),
            Err(e) => println!(
                "receiver service registration failed: {e:#}\n  sessions attach to the receiver — run `brv daemon install` later"
            ),
        }
    } else {
        println!(
            "sessions attach to this machine's receiver — register it with `brv daemon install`"
        );
    }
    println!("For unattended receiving later:");
    println!("  brv init --server … --enroll … --unattended   # or step by step:");
    println!("  brv wake set --allow respond   # unattended-session allowance (respond|edit|full)");
    println!("  brv wake test                  # verify one wake actually works");
    Ok(())
}

/// 무인 수신 셋업 — 러너 결정 → 권한 respond → 실제 깨우기 1회 → OS 서비스. 어느 단계가 막히면
/// 거기서 멈추고 이유와 다음 명령을 말한다 — 유인 모드는 이미 쓸 수 있으니 실패가 아니다.
async fn setup_unattended(
    cfg: &BrvConfig,
    runner: Option<&str>,
    interactive: bool,
) -> anyhow::Result<()> {
    let runner_id: Option<String> = match runner {
        Some(id) => Some(id.to_owned()),
        // 러너가 이미 정해져 있다 (무인 설정은 있고 서비스만 없는 재실행) — wake_set이 유지한다
        None if cfg.wake.is_some() => None,
        None => {
            let found: Vec<_> = brv::runners::detect_all()
                .into_iter()
                .filter(|d| d.spec.wake.is_some())
                .collect();
            match found.as_slice() {
                [] => {
                    println!(
                        "no agent runner found on this machine — attended use works now. For unattended receiving install one of: {} — then `brv wake set --allow respond`, `brv wake test`, `brv daemon install`",
                        brv::runners::RUNNERS
                            .iter()
                            .map(|r| r.id)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    return Ok(());
                }
                [one] => Some(one.spec.id.to_owned()),
                many => {
                    let list: Vec<String> = many
                        .iter()
                        .map(|d| format!("{} ({})", d.spec.id, d.version))
                        .collect();
                    if !interactive {
                        println!(
                            "several runners found: {} — re-run with --runner <id> to set up unattended receiving",
                            list.join(", ")
                        );
                        return Ok(());
                    }
                    println!();
                    println!("Which runner should wake this machine's agent?");
                    for (i, item) in list.iter().enumerate() {
                        println!("  [{}] {item}", i + 1);
                    }
                    let n = ask_number("choice: ", list.len())?;
                    Some(many[n - 1].spec.id.to_owned())
                }
            }
        }
    };
    println!();
    println!("setting up unattended receiving:");
    wake_set(
        Some("respond".to_owned()),
        runner_id.as_deref(),
        None,
        None,
        None,
        None,
    )?;
    // 바인딩이 여럿(한 코드에 여러 에이전트)이면 첫 바인딩으로 점검한다
    let first = cfg.bindings.first().map(|b| b.full_label());
    if let Err(e) = wake_test(first.as_deref()).await {
        println!();
        println!(
            "wake test failed: {e:#}\n  attended use still works. Fix the cause (runner login? `brv wake show`), then `brv wake test` and `brv daemon install`"
        );
        return Ok(());
    }
    println!();
    match brv::service::install(None) {
        Ok(()) => {
            println!("unattended receiving is on — messages wake the runner while you're away")
        }
        Err(e) => println!(
            "service registration failed: {e:#}\n  attended use still works — run `brv daemon install` later"
        ),
    }
    Ok(())
}

/// 예/아니오 질문 — Enter는 기본값. 터미널일 때만 부른다.
fn ask_yes_no(prompt: &str, default: bool) -> anyhow::Result<bool> {
    use std::io::Write as _;
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(match line.trim().to_ascii_lowercase().as_str() {
        "" => default,
        "y" | "yes" => true,
        _ => false,
    })
}

/// 1..=max 번호 질문 — 잘못 치면 다시 묻는다.
fn ask_number(prompt: &str, max: usize) -> anyhow::Result<usize> {
    use std::io::Write as _;
    loop {
        print!("{prompt}");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            anyhow::bail!("no input");
        }
        match line.trim().parse::<usize>() {
            Ok(n) if (1..=max).contains(&n) => return Ok(n),
            _ => println!("enter a number between 1 and {max}"),
        }
    }
}

/// Claude Code MCP 자동 등록 (기본 켬, --no-mcp로 생략) — 실패는 온보딩 실패가 아니라 안내.
/// --scope user: 기본 스코프(local)는 실행한 디렉터리에 묶여 온보딩 목적에 안 맞는다.
/// 등록에는 지금 enroll한 설정 경로를 env(BREVDUVA_CONFIG)로 박는다 — 데몬이 깨운 세션에
/// 주입하는 것과 같은 규약. 이미 등록돼 있으면 지우고 다시 등록해 **낡은 등록이 현행 설정을
/// 가리지 않게** 한다 (2026-09-01 실사고: 옛 프로필 env가 박힌 등록이 데몬 주입을 덮어 깨운
/// 세션의 MCP가 즉사 — Claude는 서버 정의 env로 상속 env를 덮는다).
/// 바인딩 수와 무관하게 등록한다 — 세션은 리시버에 붙은 뒤 `become`으로 바인딩을 고르므로(2026-09-09)
/// 옛 "바인딩이 여럿이면 프로젝트별 `--binding` 등록 안내"(페이즈 27)는 삭제했다(2026-09-13).
fn register_mcp() {
    if let Err(e) = mcp_register(None, false) {
        println!("MCP registration skipped: {e:#} — later: brv mcp register");
    }
}

/// 갱신 뒤 러너 등록을 지금 버전 형식으로 다시 쓴다 (2026-09-13, P8 — 사용자 지적 "업데이트마다 사용자가 겪는
/// 문제"): 등록은 `{brv} mcp --config … --host …` 인자를 러너 설정에 박아 두므로, 인자 형식·실행 파일 경로가 바뀐
/// 갱신은 러너 쪽 등록도 새로 써야 한다. 0.7.0 갱신 실사고 — Codex가 옛 `--binding` 등록으로 중계기를 띄워
/// "MCP startup failed: … initialize response"만 보였다(중계기의 이유는 stderr라 사용자가 못 본다). 설치기가 부르는
/// `brv daemon restart`가 이 함수를 거쳐 표시 파일(`mcp-registered.version`)이 지금 버전이 아닐 때 한 번 다시 쓴다.
/// 등록 명령이 없는 러너(조각 안내)는 여기서도 안내만 나온다 — 그 러너는 중계기가 옛 인자를 관용해 붙는다(`mcp`).
fn refresh_registrations_after_update() {
    let Ok(config_path) = config::config_path() else {
        return; // 프로필이 없으면 등록할 것도 없다
    };
    if !config_path.is_file() || !brv::service::registrations_stale(&config_path) {
        return;
    }
    println!(
        "rewriting runner MCP registrations for brv {} (an update changes what they must run)",
        env!("CARGO_PKG_VERSION")
    );
    match mcp_register(None, false) {
        Ok(()) => {}
        Err(e) => println!("  registrations not refreshed: {e:#} — later: brv mcp register"),
    }
}

/// 탐지된 러너 전부에 로컬 `brv mcp`를 등록한다 (2026-09-04 — 유인용, 깨우기 러너와 별개).
/// 러너에 등록 명령이 있으면 실행하고, 없으면 붙여 넣을 조각을 출력한다 — brv가 사용자의
/// 러너 설정 파일을 직접 고치지 않는다(형식이 제각각이라 파손 위험 > 편의).
/// 등록은 `--config`로 이 설정 파일을 못 박는다 — 러너가 MCP 자식에 환경변수를 넘기지 않아도
/// (Codex는 허용 목록만 전달) 같은 프로필을 본다.
/// 실제로 등록을 돌린 뒤에는 표시 파일에 지금 버전을 적는다 — 다음 갱신까지 `brv daemon restart`가 다시 쓰지 않는다.
fn mcp_register(runner: Option<&str>, dry_run: bool) -> anyhow::Result<()> {
    let config_path = config::config_path()?;
    let brv = std::env::current_exe().context("current exe")?;
    let targets = match runner {
        Some(id) => {
            let spec = brv::runners::spec(id).with_context(|| format!("unknown runner {id:?}"))?;
            vec![
                brv::runners::detect(spec)
                    .with_context(|| format!("{} not found on this machine", spec.display))?,
            ]
        }
        None => brv::runners::detect_all(),
    };
    if targets.is_empty() {
        println!(
            "no agent runner found on this machine — nothing to register (brv mcp register later)"
        );
        stamp(&config_path, dry_run);
        return Ok(());
    }
    for d in &targets {
        match &d.spec.mcp {
            brv::runners::McpRegistration::Command(args) => {
                let filled: Vec<String> = args
                    .iter()
                    .map(|a| brv::runners::fill(a, &brv, &config_path, d.spec.id))
                    .collect();
                let shown = format!("{} {}", d.path.display(), filled.join(" "));
                if dry_run {
                    println!("{}: would run\n  {shown}", d.spec.display);
                    continue;
                }
                let run = || std::process::Command::new(&d.path).args(&filled).output();
                match run() {
                    Ok(out) if out.status.success() => {
                        println!("{}: brevduva MCP registered", d.spec.display);
                    }
                    Ok(out) => {
                        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
                        let already = err.to_ascii_lowercase().contains("already");
                        // Claude Code는 지우고 다시 등록한다 — 다른 설정 경로로 재init한 머신이 옛 등록을 물고 있던
                        // 실사고(2026-09-02)의 대응이고, 갱신 뒤 자동 재등록(2026-09-13)도 이 길로 새 인자를 쓴다.
                        // Codex는 `mcp add`가 같은 이름을 덮어써(0.153.4 실측) 이 가지에 오지 않는다. 다른 러너는
                        // remove 문법을 실측 전이라 안내만
                        if already && d.spec.id == "claude" {
                            let removed = std::process::Command::new(&d.path)
                                .args(["mcp", "remove", "brevduva", "-s", "user"])
                                .output()
                                .map(|o| o.status.success())
                                .unwrap_or(false);
                            match run() {
                                Ok(out) if removed && out.status.success() => println!(
                                    "{}: brevduva MCP registration refreshed — it now points at this config",
                                    d.spec.display
                                ),
                                _ => println!(
                                    "{}: already registered but could not be refreshed — by hand: claude mcp remove brevduva -s user && {shown}",
                                    d.spec.display
                                ),
                            }
                        } else if already {
                            println!(
                                "{}: a `brevduva` MCP entry already exists — left as is. If it is the remote connector (url = https://brevduva.dev/mcp), rename that entry (e.g. brevduva-remote) and re-run `brv mcp register`, so the local receiver owns the name",
                                d.spec.display
                            );
                        } else {
                            println!(
                                "{}: registration failed ({}) — by hand:\n  {shown}",
                                d.spec.display,
                                err.lines().next().unwrap_or("no error output")
                            );
                        }
                    }
                    Err(e) => println!(
                        "{}: could not run its CLI ({e}) — by hand:\n  {shown}",
                        d.spec.display
                    ),
                }
            }
            brv::runners::McpRegistration::Snippet { file, body } => {
                println!(
                    "{}: no registration command — add this to {file}:\n{}\n",
                    d.spec.display,
                    brv::runners::fill(body, &brv, &config_path, d.spec.id)
                );
            }
        }
    }
    stamp(&config_path, dry_run);
    Ok(())
}

/// 등록을 실제로 돌렸으면 표시 파일에 지금 버전을 적는다(미리 보기는 제외).
fn stamp(config_path: &std::path::Path, dry_run: bool) {
    if !dry_run && let Err(e) = brv::service::stamp_registrations(config_path) {
        println!("  (could not record the registration version: {e})");
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
    // 기존 설정에는 바인딩을 upsert — 다른 바인딩과 [wake]가 보존된다 (페이즈 27).
    // 관리 키 경로는 org를 모른다(서버 default_org 소관) — 구형과 같은 org 미상 바인딩
    let new_binding = Binding {
        org: None,
        agent: agent.clone(),
        channel,
        description,
        wake_dir: None,
        wake_command: None,
        wake_args: None,
    };
    let (cfg, replaced) = merge_binding(&base, new_binding.clone())?;
    let stored = config::store_token(&cfg.server, &new_binding, &token)?;
    let path = config::store(&cfg)?;

    println!(
        "initialized{} — config: {path:?} ({})",
        if replaced {
            " (existing binding updated)"
        } else {
            ""
        },
        token_store_note(&stored)
    );
    println!();
    println!("to connect Claude Code:");
    println!("  claude mcp add brevduva -- brv mcp");
    println!();
    println!("manual checks: brv status / brv listen / brv send --to <agent> --payload \"...\"");
    Ok(())
}

/// `brv binding list` — 바인딩 목록 + 토큰 유무·깨우기 설정.
fn binding_list() -> anyhow::Result<()> {
    let cfg = config::load()?;
    println!("server {} — {} binding(s):", cfg.server, cfg.bindings.len());
    for b in &cfg.bindings {
        let token = if config::load_token(&cfg, b).is_ok() {
            "token ok"
        } else {
            "token MISSING — enroll needed"
        };
        println!(
            "  {:34} {token:28} dir {}",
            b.full_label(),
            b.wake_dir.as_deref().unwrap_or("(unset)")
        );
        if !b.description.is_empty() {
            println!("    {}", b.description);
        }
    }
    if cfg.bindings.is_empty() {
        println!("  (none — connect with brv init --enroll <code>)");
    }
    Ok(())
}

/// `brv binding add` — 기존 토큰으로 바인딩 추가. grant는 채널 발견(10.2)으로 선검증 —
/// 없는 채널·미부여 채널이 조용히 죽은 바인딩으로 남지 않게 한다. 발견 응답의 org를
/// 새 바인딩에 채운다 (2026-09-01 — 조직 간 동명 구분의 진실은 서버).
async fn binding_add(agent: String, channel: String, description: String) -> anyhow::Result<()> {
    let cfg = config::load()?;
    // 토큰 조회용 프로브 — 같은 에이전트의 기존 바인딩이 있으면 그 org 기준으로 찾는다
    let probe = cfg
        .bindings
        .iter()
        .find(|b| b.agent == agent)
        .cloned()
        .unwrap_or(Binding {
            org: None,
            agent: agent.clone(),
            channel: String::new(),
            description: String::new(),
            wake_dir: None,
            wake_command: None,
            wake_args: None,
        });
    let token = config::load_token(&cfg, &probe).with_context(|| {
        format!(
            "no token for agent {agent:?} on this machine — connect with `brv init --enroll` first"
        )
    })?;
    let (org, _, granted) = brv::client::discover_channels(&cfg.server, &token).await?;
    anyhow::ensure!(
        granted.contains(&channel),
        "agent {agent:?} has no grant for channel {channel:?} — grant it in the dashboard (granted: {})",
        granted.join(", ")
    );
    let (cfg, replaced) = merge_binding(
        &cfg.server.clone(),
        Binding {
            org: (!org.is_empty()).then_some(org),
            agent,
            channel,
            description,
            wake_dir: None,
            wake_command: None,
            wake_args: None,
        },
    )?;
    let path = config::store(&cfg)?;
    println!(
        "binding {} — {path:?}",
        if replaced { "updated" } else { "added" }
    );
    restart_daemon(false)?;
    Ok(())
}

/// `brv daemon pause --for <기간>` (2026-09-03) — 대화형 세션이 채널을 직접 맡는 동안 데몬이
/// 자리를 비운다 (메시지는 서버 큐에). 파일 신호라 데몬 재기동 없이 5초 안에 반영된다.
fn pause_daemon(spec: &str) -> anyhow::Result<()> {
    let secs = parse_duration_secs(spec)?;
    brv::daemon::write_pause(brv::daemon::now_unix() + secs)?;
    println!(
        "daemon paused for {spec} — it leaves the channel within a few seconds; messages queue server-side. End early: brv daemon resume"
    );
    Ok(())
}

fn resume_daemon() -> anyhow::Result<()> {
    if brv::daemon::clear_pause()? {
        println!("daemon resumed — it re-checks wake and rejoins the channel within a few seconds");
    } else {
        println!("daemon was not paused");
    }
    Ok(())
}

/// "30s" / "45m" / "2h" — 단위 없는 숫자는 분.
fn parse_duration_secs(spec: &str) -> anyhow::Result<u64> {
    let spec = spec.trim();
    let (num, unit) = spec
        .find(|c: char| !c.is_ascii_digit())
        .map_or((spec, "m"), |i| spec.split_at(i));
    let n: u64 = num
        .parse()
        .with_context(|| format!("invalid duration {spec:?} — use e.g. 30m, 2h"))?;
    let secs = match unit.trim() {
        "s" => n,
        "m" | "min" => n * 60,
        "h" => n * 3600,
        _ => anyhow::bail!("invalid duration unit in {spec:?} — use s, m, or h"),
    };
    anyhow::ensure!(secs > 0, "duration must be positive");
    Ok(secs)
}

/// 설정을 바꾼 명령들이 부른다 (2026-09-02): 서비스가 등록돼 있으면 재기동해 변경을 즉시 반영,
/// 아니면 직접 재시작하라고 안내. explicit(`brv daemon restart`)면 미등록을 오류로 돌려준다.
fn restart_daemon(explicit: bool) -> anyhow::Result<()> {
    // 갱신이 서비스에 닿게 (2026-09-04): 설치기는 CLI 경로만 바꾸는데 서비스가 다른 경로로
    // 등록돼 있으면 재기동해도 옛 코드가 다시 뜬다 — 사용자가 파일을 손으로 복사할 일이 아니다
    if let Some((path, previous)) = brv::service::align_binary() {
        println!(
            "service binary updated: {} ({previous} → brv {})",
            path.display(),
            env!("CARGO_PKG_VERSION")
        );
    }
    let restarted = brv::service::restart();
    match &restarted {
        Ok(true) => println!("daemon restarted (OS service) — changes are live"),
        Ok(false) if explicit => {}
        // 서비스는 없지만 데몬이 돌았던 흔적(상태 파일)이 있으면 — 직접 띄운 데몬·작업 스케줄러 등
        Ok(false) if brv::daemon::read_state().is_some() => println!(
            "daemon is not an OS service here — if one is running, restart it yourself so the change applies"
        ),
        Ok(false) => {}
        Err(e) => println!("daemon restart failed — restart it yourself: {e}"),
    }
    // 갱신 뒤처리는 서비스 유무와 무관하게 한다 (2026-09-13) — 설치기는 이 명령 하나만 부르므로 여기서 끝나야
    // 사용자가 손댈 것이 없다(P8). ① 설치기가 이 실행 파일 옆에 비켜 둔 옛 파일 정리(10단계) ② 러너 등록 다시 쓰기
    if let Ok(exe) = std::env::current_exe() {
        let removed = brv::service::sweep_parked_binaries(&exe);
        if !removed.is_empty() {
            println!(
                "removed {} binary file(s) left over from a previous update",
                removed.len()
            );
        }
    }
    refresh_registrations_after_update();
    if explicit && matches!(restarted, Ok(false)) {
        anyhow::bail!(
            "daemon is not registered as an OS service — restart the process you started yourself, or register one with `brv daemon install`"
        );
    }
    Ok(())
}

/// `brv binding remove` — 바인딩 제거. 토큰은 의도적으로 남긴다 (같은 에이전트의 다른
/// 바인딩·재추가가 쓸 수 있음 — 회수는 대시보드의 토큰 회전이 담당).
fn binding_remove(selector: &str) -> anyhow::Result<()> {
    let mut cfg = config::load()?;
    // full_label이 정체성 키 — 같은 label이라도 org가 다르면 다른 바인딩 (2026-09-01)
    let found = cfg.find(selector)?;
    let (full, token_id) = (found.full_label(), found.token_id());
    cfg.bindings.retain(|b| b.full_label() != full);
    let path = config::store(&cfg)?;
    println!("binding {full} removed — {path:?}");
    if !cfg.bindings.iter().any(|b| b.token_id() == token_id) {
        println!(
            "  (the agent's token stays where it is stored — keychain or token file; to revoke it, revoke the connection in the dashboard)"
        );
    }
    restart_daemon(false)?;
    Ok(())
}

async fn status(binding_sel: Option<&str>) -> anyhow::Result<()> {
    let cfg = config::load()?;
    println!(
        "profile: {} ({})",
        config::config_path()?.display(),
        config::profile_source()
    );
    println!(
        "config: server {} / {} binding(s)",
        cfg.server,
        cfg.bindings.len()
    );
    for b in &cfg.bindings {
        println!("  {}", b.label());
    }
    // 러너 탐지 (2026-09-04) — 이 머신에서 깨울 수 있는 CLI 에이전트와 실제 경로·버전.
    // 사용자가 "왜 codex를 못 찾지"를 물을 때 답이 여기 있어야 한다
    let runners = brv::runners::detect_all();
    if runners.is_empty() {
        println!(
            "runners: none found — attended use only. For unattended wake install one of: {}",
            brv::runners::RUNNERS
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>()
                .join(", ")
        );
    } else {
        println!("runners:");
        for d in &runners {
            // 세 칸 (RECEIVER_DESIGN §3, 2026-09-12 11단계): 무인 깨우기 / CLI 유인 밀어넣기 / GUI 유인 밀어넣기를
            // 실측 여부와 함께 — 서버 리포 RUNNERS.md와 같은 표다
            println!("  {:10} {:26} {}", d.spec.id, d.version, d.path.display());
            println!(
                "             wake: {} · CLI push: {} · GUI push: {}",
                d.spec.wake_capability(),
                d.spec.attended_cli.describe(),
                d.spec.attended_gui.describe()
            );
        }
    }
    // 데몬 상태 파일 (2026-09-02) — "idle인지 죽었는지"를 프레즌스가 아니라 데몬 자신이 답한다
    match brv::daemon::read_state() {
        Some(daemon) => {
            println!(
                "daemon: pid {} (state updated {}s ago)",
                daemon.pid,
                daemon.age_secs()
            );
            for (label, st) in &daemon.bindings {
                println!(
                    "  {:34} {} — for {}s{}",
                    label,
                    st.describe(),
                    st.age_secs(),
                    st.wake_check
                        .as_ref()
                        .map(|w| format!("; wake pre-flight {w}"))
                        .unwrap_or_default()
                );
            }
        }
        None => println!("daemon: no state file (not running here, or older than 0.6.6)"),
    }
    // 이전 리시버가 서버에 확정했지만 넘기지 못한 메시지 (10단계) — 종전에는 리시버 로그에만 있었다
    for leftover in brv::daemon::legacy_leftovers(&cfg) {
        match &leftover.unfinished {
            Ok(messages) => println!(
                "previous receiver: {} ({}) — {} message(s) were confirmed to the server but never handed over; they are not re-delivered. Inspect {}",
                leftover.binding,
                leftover.adapter,
                messages.len(),
                leftover.journal.display()
            ),
            Err(error) => println!(
                "previous receiver: could not read {} — {error}",
                leftover.journal.display()
            ),
        }
    }
    if let Some(until) = brv::daemon::read_pause() {
        println!(
            "daemon: PAUSED by operator — {} min left (brv daemon resume ends it early)",
            until.saturating_sub(brv::daemon::now_unix()).div_ceil(60)
        );
    }
    let health = reqwest::Client::new()
        .get(format!("{}/healthz", cfg.server.trim_end_matches('/')))
        .send()
        .await;
    match health {
        Ok(resp) if resp.status().is_success() => println!("server: OK"),
        Ok(resp) => println!("server: unexpected response ({})", resp.status()),
        Err(e) => {
            println!("server: unreachable — {e}");
            return Ok(());
        }
    }
    // 조회는 **리시버에게 묻는다** (2026-09-09, P2): 종전에는 이 명령이 자기 토큰으로 JOIN해
    // 프레즌스를 물었고 그 JOIN이 데몬·대화형 세션의 자리를 빼앗았다(2026-09-08 실측).
    // 이제 서버에 붙는 것은 리시버뿐이고, 여기서는 이미 붙어 있는 그 접속의 답을 받는다.
    report_local_plane(binding_sel).await;
    Ok(())
}

/// 로컬 리시버에게 세션·바인딩·프레즌스를 묻는다. 리시버가 없으면 그 사실만 알린다.
async fn report_local_plane(binding_sel: Option<&str>) {
    let endpoint = match brv::local_plane::Endpoint::load() {
        Ok(endpoint) => endpoint,
        Err(_) => {
            println!(
                "local sessions: no receiver endpoint on this machine — sessions cannot attach. Register the service with `brv daemon install`"
            );
            return;
        }
    };
    let request = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("http client")
        .get(format!("http://{}/status", endpoint.addr))
        .bearer_auth(endpoint.token.expose())
        .timeout(Duration::from_secs(20))
        .send()
        .await;
    let report: serde_json::Value = match request {
        Ok(response) if response.status().is_success() => match response.json().await {
            Ok(value) => value,
            Err(e) => {
                println!("local sessions: receiver answered but the report was unreadable — {e}");
                return;
            }
        },
        Ok(response) => {
            println!(
                "local sessions: receiver refused the status query ({})",
                response.status()
            );
            return;
        }
        Err(e) => {
            println!("local sessions: receiver endpoint unreachable — {e}");
            return;
        }
    };
    if report["receiver_version"].as_str() != Some(env!("CARGO_PKG_VERSION")) {
        println!(
            "local sessions: receiver is {} but this CLI is {} — restart the service so both match",
            report["receiver_version"].as_str().unwrap_or("unknown"),
            env!("CARGO_PKG_VERSION")
        );
    }
    let sessions = report["sessions"].as_array().cloned().unwrap_or_default();
    if sessions.is_empty() {
        println!("local sessions: none attached");
    } else {
        println!("local sessions:");
        for session in &sessions {
            println!(
                "  {:10} {:8} {:9} {}",
                session["host"].as_str().unwrap_or("unknown"),
                session["origin"].as_str().unwrap_or("?"),
                if session["receiving"] == serde_json::json!(true) {
                    "receiving"
                } else {
                    "no-push"
                },
                session["bindings"]
                    .as_array()
                    .map(|b| b
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            );
        }
    }
    for binding in report["bindings"].as_array().unwrap_or(&Vec::new()) {
        let label = binding["binding"].as_str().unwrap_or_default();
        if binding_sel.is_some_and(|sel| !label.contains(sel)) {
            continue;
        }
        let held = binding["held_by_work"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        println!(
            "  binding {label}: {}{}{}",
            if binding["connected"] == serde_json::json!(true) {
                "connected"
            } else {
                "not connected"
            },
            match binding["holder"].as_str() {
                Some(_) if binding["receiving"] == serde_json::json!(true) =>
                    ", a session is receiving",
                Some(_) => ", held by a session that cannot receive",
                None => ", no session holds it (deliveries wake one)",
            },
            if held.is_empty() {
                String::new()
            } else {
                format!(
                    ", locked by work {}",
                    held.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        );
        if let Some(entries) = report["presence"][label].as_array() {
            let listening: Vec<String> = entries
                .iter()
                .map(|e| {
                    format!(
                        "{} {}",
                        e["agent"].as_str().unwrap_or("?"),
                        e["state"].as_str().unwrap_or("?")
                    )
                })
                .collect();
            if !listening.is_empty() {
                println!("    channel presence: {}", listening.join(", "));
            }
        }
    }
}

async fn send(
    to: String,
    payload: String,
    expects_ack: bool,
    reply_to: Option<String>,
    binding_sel: Option<&str>,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    // 리시버에게 맡긴다 (2026-09-10, P2·8단계): 종전에는 이 명령이 자기 토큰으로 JOIN해 보내며
    // 데몬·대화형 세션의 자리를 잠깐씩 빼앗았다. 이제 서버에 붙는 것은 리시버뿐이다.
    // --reply-to (2026-09-02, 실사용 보고)는 그대로다 — kind=reply + correlation으로 보낸다.
    let cfg = config::load()?;
    let binding = cfg.select(binding_sel)?;
    let endpoint = brv::local_plane::Endpoint::load().context(
        "no local receiver is running on this machine — start it with `brv daemon install`; the CLI no longer connects to the server itself",
    )?;
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()?
        .post(format!("http://{}/publish", endpoint.addr))
        .bearer_auth(endpoint.token.expose())
        .timeout(Duration::from_secs(20))
        .json(&serde_json::json!({
            "binding": binding.full_label(),
            "to": to,
            "payload": payload,
            "expects_ack": expects_ack,
            "reply_to": reply_to,
            // 리시버가 깨운 프로세스면 그 깨우기로 받았다는 증거가 된다 (2026-09-11)
            "wake": std::env::var("BREVDUVA_WAKE").ok().filter(|w| !w.is_empty()),
        }))
        .send()
        .await
        .context("the local receiver did not answer")?;
    anyhow::ensure!(
        response.status().is_success(),
        "the local receiver refused the publish ({})",
        response.status()
    );
    let result: serde_json::Value = response.json().await?;
    match result["status"].as_str() {
        Some("sent") => println!("sent {}", result["id"].as_str().unwrap_or_default()),
        Some("unconfirmed") => {
            anyhow::bail!("unconfirmed after 10s — the receiver republishes on reconnect (13.3)")
        }
        _ => anyhow::bail!(
            "{}",
            result["message"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| result.to_string())
        ),
    }
    Ok(())
}

/// 리시버 관찰 (2026-09-11 확정, 17단계) — 리시버의 `/listen`을 읽기만 한다. 옛 `listen`은 서버에 JOIN해 받은
/// 메시지를 소비했다: 켜는 순간 리시버와 수신 자리를 다투고, 받은 것은 에이전트에게 가지 않았다(P2 위반).
/// 리시버가 없으면 그렇다고 말하고 끝난다 — 서버 직접 접속으로 대신하지 않는다.
async fn listen(binding: Option<&str>, raw: bool) -> anyhow::Result<()> {
    let endpoint = brv::local_plane::auth::Endpoint::load().context(
        "no local receiver is running on this machine — `brv listen` watches the receiver; start it with `brv daemon install` (or `brv daemon`)",
    )?;
    let http = reqwest::Client::builder().no_proxy().build()?;
    let mut response = http
        .get(format!("http://{}/listen", endpoint.addr))
        .bearer_auth(endpoint.token.expose())
        .send()
        .await
        .context("the local receiver did not answer")?;
    anyhow::ensure!(
        response.status().is_success(),
        "the local receiver refused the watch stream ({}) — is it an older version? restart it after updating",
        response.status()
    );
    eprintln!(
        "watching the receiver at {} — nothing is taken; Ctrl+C to stop",
        endpoint.addr
    );
    // 줄 경계는 바이트로 자른다 — 조각 경계에 걸린 여러 바이트 글자가 깨지지 않게.
    let mut buffer: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.context("the watch stream broke")? {
        buffer.extend_from_slice(&chunk);
        while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = buffer.drain(..=end).collect();
            let Ok(event) = serde_json::from_slice::<serde_json::Value>(line.trim_ascii()) else {
                continue; // 유휴 줄
            };
            if let (Some(selector), Some(bound)) = (binding, event["binding"].as_str())
                && bound != selector
                && !bound.ends_with(&format!("/{selector}"))
            {
                continue;
            }
            if raw {
                println!("{event}");
            } else {
                println!("{}", describe_tap(&event));
            }
        }
    }
    eprintln!("the receiver closed the watch stream (it stopped or restarted)");
    Ok(())
}

/// 관찰 사건 한 줄 — 사람이 읽는 형태. 시각은 UTC.
fn describe_tap(event: &serde_json::Value) -> String {
    let text = |key: &str| event[key].as_str().unwrap_or_default().to_owned();
    let when = event["at_ms"]
        .as_u64()
        .map(|ms| {
            let secs = ms / 1000;
            format!(
                "{:02}:{:02}:{:02}Z",
                (secs / 3600) % 24,
                (secs / 60) % 60,
                secs % 60
            )
        })
        .unwrap_or_default();
    let binding = text("binding");
    let what = match text("event").as_str() {
        "received" => {
            let whither = match text("route").as_str() {
                "handed_to_session" => format!("→ session {}", text("session")),
                "held_for_manual_receive" => {
                    format!("→ held for manual receive by session {}", text("session"))
                }
                "unattended" => "→ unattended path".to_owned(),
                "deferred" => format!("→ deferred {}s: {}", event["delay_s"], text("reason")),
                "consumed" => format!("→ handled by the receiver: {}", text("reason")),
                other => format!("→ {other}"),
            };
            let (kind, id, from) = (text("kind"), text("message_id"), text("from"));
            let preview = text("preview").replace('\n', " ");
            if preview.is_empty() {
                format!("{kind} {id} from {from} {whither}")
            } else {
                format!("{kind} {id} from {from} {whither}\n    {preview}")
            }
        }
        "accepted" => format!(
            "{} confirmed — the agent received it ({})",
            text("message_id"),
            text("via")
        ),
        "uncertain" => format!(
            "{} outcome uncertain — {} (decide with receiver_resolve)",
            text("message_id"),
            text("reason")
        ),
        "wake_started" => format!(
            "woke a session for {} (wake {})",
            event["message_ids"],
            text("wake")
        ),
        "wake_proven" => format!(
            "the woken session proved it received the batch — confirmed (wake {})",
            text("wake")
        ),
        "wake_unproven" => format!(
            "the woken session ended without proving receipt (wake {})",
            text("wake")
        ),
        "wake_failed" => format!("a wake could not start: {}", text("reason")),
        "deferred" => format!(
            "{} deferred {}s: {}",
            event["message_ids"],
            event["delay_s"],
            text("reason")
        ),
        "lagged" => format!(
            "… {} events were dropped (this watcher fell behind)",
            event["missed"]
        ),
        other => other.to_owned(),
    };
    format!("{when} {binding} {what}")
}

async fn mcp(binding_sel: Option<&str>, host: Option<String>) -> anyhow::Result<()> {
    // **로컬 리시버로의 브리지**다 (2026-09-09, RECEIVER_DESIGN P2·P3): 이 프로세스는 서버에
    // JOIN하지 않고 바인딩을 고르지도 않는다 — 정체성은 리시버의 등록부가 정한다. 그래서 바인딩이
    // 여럿인 머신에서도 대화형 세션이 그대로 붙는다(0.6.39 검토 1번의 근본 수정). 러너별 전달
    // 어댑터(`--claude-channel`·`--codex-cli-endpoint`)는 리시버 소유로 옮겨져 삭제됐다(2026-09-11, 7e).
    // 옛 등록의 `--binding`은 무시하고 붙는다 (2026-09-13 번복 — 종전엔 이유를 말하고 종료): 이 프로세스는 지금
    // 버전의 중계기이고 낡은 것은 러너 설정의 인자뿐이다. 거부하면 러너(Codex)는 stderr를 삼켜 "MCP startup failed"만
    // 보이고, 등록을 새로 쓰는 것은 갱신(`brv daemon restart`)의 몫이다 — 그것이 닿지 못한 등록(조각 안내 러너·손 편집)
    // 때문에 사용자가 멈춰서는 안 된다. 정체성은 여전히 `become`이 정한다.
    if let Some(binding) = binding_sel {
        eprintln!(
            "brv mcp: ignoring --binding {binding} from a registration older than 0.7.0 — sessions take an identity with `become`; `brv mcp register` rewrites this runner's entry"
        );
    }
    brv::local_plane::bridge::run(host).await
}
