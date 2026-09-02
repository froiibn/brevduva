// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! brv — Brevduva 리시버 CLI.
//!
//! `init`(셋업 일괄) · `binding`(다중 바인딩 관리) · `status` · `send` · `listen`(수신 출력) ·
//! `mcp`(로컬 MCP 서버) · `daemon`(상주 수신+깨우기) · `wake` · `hook`.
//!
//! **다중 바인딩 (페이즈 27)**: 설정은 여러 (에이전트, 채널) 바인딩을 담고, 데몬은 전부
//! 동시 수신한다. 단일 대상 명령(mcp·send·listen·status·channels·wake test)은 바인딩이
//! 하나면 그것, 여럿이면 `--binding {agent}@{channel}` 명시를 요구한다 — 조용한 오발신 방지.

use std::time::Duration;

use anyhow::Context as _;
use brv::client::{Client, ClientOptions, PublishSpec, RecvFilter};
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
        /// Skip the automatic Claude Code MCP registration after enroll
        #[arg(long)]
        no_mcp: bool,
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
    /// Print received messages as line-delimited JSON (for manual testing, Ctrl+C to stop)
    Listen {
        /// Receiving binding — required when multiple bindings exist
        #[arg(long)]
        binding: Option<String>,
    },
    /// Local MCP server (stdio) — for MCP hosts such as Claude Code
    Mcp {
        /// Binding for this session — required when multiple bindings exist (pin it in each project's .mcp.json)
        #[arg(long)]
        binding: Option<String>,
    },
    /// Resident daemon — receives on all bindings at once and wakes a session per message (needs [wake] in config)
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
        /// Wake executable — if omitted, finds claude on PATH and stores its absolute path.
        /// With --binding, overrides the runner for that binding only (e.g. codex); otherwise global
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
    /// Register as an OS service (linux=systemd user unit, macOS=launchd, windows=SCM service)
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
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // 윈도우 서비스 모드: 콘솔이 없다 — 로그는 설정 디렉터리 파일로, 설정 경로는
    // SCM launch args에서 (에디션 2024의 unsafe set_var 대신 프로세스 내 override).
    // 런타임 진입 전에 분기 — SCM dispatcher는 자기 스레드를 점유한다
    if let Cmd::Daemon {
        action: Some(DaemonCmd::ServiceRun { config }),
        ..
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
        Cmd::Listen { binding } => listen(binding.as_deref()).await,
        Cmd::Mcp { binding } => mcp(binding.as_deref()).await,
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
                command,
                dir,
                timeout,
                binding,
            } => wake_set(allow, command, dir, timeout, binding.as_deref()),
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

/// 선택된 바인딩의 접속 옵션 — 단일 대상 명령들의 공통 진입.
fn options_from_config(
    selector: Option<&str>,
) -> anyhow::Result<(BrvConfig, Binding, ClientOptions)> {
    let cfg = config::load()?;
    let binding = cfg.select(selector)?.clone();
    let token = config::load_token(&cfg, &binding)?;
    let mut opts = ClientOptions::new(&cfg.server, &binding.channel, &binding.agent, token);
    opts.description = binding.description.clone();
    Ok((cfg, binding, opts))
}

fn connect_from_config(selector: Option<&str>) -> anyhow::Result<(Binding, Client)> {
    let (_, binding, opts) = options_from_config(selector)?;
    Ok((binding, Client::connect(opts)))
}

/// PATH에서 실행 파일 탐색 — 설정에는 항상 절대 경로로 저장하기 위함.
/// 2026-08-29 실사고의 교훈: 서비스(systemd 등) 환경의 PATH에는 사용자 설치 경로가 없어
/// 상대 이름 "claude"가 안 풀렸다. 설정 시점에 절대 경로로 못 박으면 재발하지 않는다.
fn find_in_path(name: &str) -> Option<std::path::PathBuf> {
    let exts: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| {
        exts.iter().find_map(|ext| {
            let p = dir.join(format!("{name}{ext}"));
            p.is_file().then_some(p)
        })
    })
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
    command: Option<String>,
    dir: Option<String>,
    timeout: Option<u64>,
    binding_sel: Option<&str>,
) -> anyhow::Result<()> {
    let mut cfg = config::load()?; // 연결 설정(init) 위에 얹는다 — 미init이면 여기서 안내됨
    // --binding이 있으면 --command/--allow는 그 바인딩의 오버라이드 (2026-09-01 러너 혼용)
    let binding_scoped = binding_sel.is_some();
    if let Some(level) = &allow {
        anyhow::ensure!(
            config::wake_preset_args(level).is_some(),
            "unknown --allow {level:?} — one of: respond, edit, full"
        );
    }

    // ---- 전역부: 바인딩 스코프가 아닐 때의 --command/--allow + 항상 전역인 --timeout ----
    let existing = cfg.wake.take();
    let g_command = match (&command, binding_scoped) {
        (Some(c), false) => resolve_command(c.clone())?,
        _ => match &existing {
            Some(w) => w.command.clone(),
            None => find_in_path("claude")
                .context(
                    "claude not found in PATH — install Claude Code CLI, \
                     or point --command at another agent runner (absolute path)",
                )?
                .to_string_lossy()
                .into_owned(),
        },
    };
    let g_args = match (&allow, binding_scoped) {
        (Some(level), false) => config::wake_preset_args(level).expect("validated above"),
        _ => match &existing {
            Some(w) => w.args.clone(),
            None => config::wake_preset_args("respond").expect("respond preset exists"),
        },
    };
    let timeout_s = timeout
        .or(existing.as_ref().map(|w| w.timeout_s))
        .unwrap_or(600);
    cfg.wake = Some(config::WakeConfig {
        command: g_command,
        args: g_args,
        timeout_s,
    });

    // ---- 바인딩부 (페이즈 27): dir + 스코프된 command/allow ----
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
            if let Some(c) = command {
                target.wake_command = Some(resolve_command(c)?);
            }
            if let Some(level) = &allow {
                target.wake_args = Some(config::wake_preset_args(level).expect("validated above"));
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
    let level = config::wake_preset_of(&wake.args).unwrap_or("custom (hand-edited args)");
    println!("allow    : {level} (global)");
    println!(
        "tools    : {}",
        config::wake_allowed_tools(&wake.args).unwrap_or("(no --allowedTools in args)")
    );
    println!("command  : {} (global)", wake.command);
    println!("timeout  : {}s", wake.timeout_s);
    println!("bindings :");
    for b in &cfg.bindings {
        let eff = brv::daemon::effective_wake(wake, b);
        let runner = if b.wake_command.is_some() || b.wake_args.is_some() {
            format!("runner {} {} (override)", eff.command, eff.args.join(" "))
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
    let child = brv::daemon::spawn_wake(&capped, &dir, &binding.full_label(), prompt).await?;
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
        println!("to use from an agent: claude mcp add brevduva -- brv mcp");
    } else {
        register_mcp(&cfg);
    }
    // 돌고 있는 데몬에 새 토큰·바인딩을 즉시 반영 (2026-09-02 맥북 실사고 — 재enroll 후 재기동
    // 없이는 데몬이 옛 토큰으로 죽어 있었다). 서비스 미등록이면 조용히 지나간다
    restart_daemon(false)?;
    // 온보딩 가이드 체인 (2026-09-02 사용자 확정): 마법사 대신 각 단계의 완료
    // 메시지가 다음 단계를 안내한다 — 설치기는 enroll을, enroll은 무인 모드를.
    println!();
    println!(
        "to receive while you're away (optional) — this machine wakes an agent session per message:"
    );
    println!("  brv wake set --allow respond   # unattended-session allowance (respond|edit|full)");
    println!("  brv wake test                  # verify one wake actually works");
    println!("  brv daemon install             # register the resident OS service");
    Ok(())
}

/// Claude Code MCP 자동 등록 (기본 켬, --no-mcp로 생략) — 실패는 온보딩 실패가 아니라 안내.
/// --scope user: 기본 스코프(local)는 실행한 디렉터리에 묶여 온보딩 목적에 안 맞는다.
/// 등록에는 지금 enroll한 설정 경로를 env(BREVDUVA_CONFIG)로 박는다 — 데몬이 깨운 세션에
/// 주입하는 것과 같은 규약. 이미 등록돼 있으면 지우고 다시 등록해 **낡은 등록이 현행 설정을
/// 가리지 않게** 한다 (2026-09-01 실사고: 옛 프로필 env가 박힌 등록이 데몬 주입을 덮어 깨운
/// 세션의 MCP가 즉사 — Claude는 서버 정의 env로 상속 env를 덮는다).
/// 바인딩이 여럿이면 자동 등록하지 않는다 — 전역 `brv mcp`는 바인딩 선택이 안 되므로,
/// 프로젝트별 등록(--binding 포함)을 안내한다 (페이즈 27).
fn register_mcp(cfg: &BrvConfig) {
    if cfg.bindings.len() > 1 {
        println!(
            "multiple bindings — skipping global MCP auto-registration. In each project directory:"
        );
        for b in &cfg.bindings {
            println!(
                "  claude mcp add brevduva -- brv mcp --binding {}",
                b.label()
            );
        }
        return;
    }
    let config_env = config::config_path()
        .map(|p| format!("BREVDUVA_CONFIG={}", p.display()))
        .ok();
    let add = || {
        let mut cmd = std::process::Command::new("claude");
        cmd.args(["mcp", "add", "--scope", "user"]);
        if let Some(env) = &config_env {
            cmd.args(["--env", env]);
        }
        cmd.args(["brevduva", "--", "brv", "mcp"]).output()
    };
    match add() {
        Ok(out) if out.status.success() => {
            println!("Claude Code MCP registered — the agent can use the channel right away");
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            if err.contains("already exists") {
                let removed = std::process::Command::new("claude")
                    .args(["mcp", "remove", "brevduva", "-s", "user"])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                match add() {
                    Ok(out) if removed && out.status.success() => {
                        println!(
                            "Claude Code MCP registration refreshed — it now points at this config"
                        );
                    }
                    _ => println!(
                        "Claude Code MCP is already registered but could not be refreshed — redo manually: claude mcp remove brevduva -s user && claude mcp add --scope user brevduva -- brv mcp"
                    ),
                }
            } else {
                println!(
                    "MCP auto-registration failed — register manually: claude mcp add brevduva -- brv mcp\n  ({})",
                    err.trim()
                );
            }
        }
        Err(_) => {
            println!(
                "claude CLI not found — skipping MCP registration. To use from an agent: claude mcp add brevduva -- brv mcp"
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
    match brv::service::restart() {
        Ok(true) => println!("daemon restarted (OS service) — changes are live"),
        Ok(false) if explicit => anyhow::bail!(
            "daemon is not registered as an OS service — restart the process you started yourself, or register one with `brv daemon install`"
        ),
        // 서비스는 없지만 데몬이 돌았던 흔적(상태 파일)이 있으면 — 직접 띄운 데몬·작업 스케줄러 등
        Ok(false) if brv::daemon::read_state().is_some() => println!(
            "daemon is not an OS service here — if one is running, restart it yourself so the change applies"
        ),
        Ok(false) => {}
        Err(e) => println!("daemon restart failed — restart it yourself: {e}"),
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
            "  (the agent's token stays in the keychain — to revoke it, rotate the token in the dashboard)"
        );
    }
    restart_daemon(false)?;
    Ok(())
}

async fn status(binding_sel: Option<&str>) -> anyhow::Result<()> {
    let cfg = config::load()?;
    println!(
        "config: server {} / {} binding(s)",
        cfg.server,
        cfg.bindings.len()
    );
    for b in &cfg.bindings {
        println!("  {}", b.label());
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
    // 프레즌스 조회는 JOIN을 동반한다(순간 online 표시) — 전 바인딩 순회는 프레즌스 노이즈라
    // 단일 결정이 가능할 때만 (바인딩 1개 또는 --binding 지정)
    match cfg.select(binding_sel) {
        Ok(binding) => {
            let (_, client) = connect_from_config(Some(&binding.label()))?;
            match client.presence(Duration::from_secs(10)).await {
                Ok(entries) => {
                    println!("channel {} presence:", binding.channel);
                    for e in entries {
                        println!("  {:10} {:?}", e.agent.as_str(), e.state);
                    }
                }
                Err(e) => println!("presence query failed: {e}"),
            }
        }
        Err(_) if !cfg.bindings.is_empty() => {
            println!("(channel presence is shown with --binding {{agent}}@{{channel}})");
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

async fn send(
    to: String,
    payload: String,
    expects_ack: bool,
    reply_to: Option<String>,
    binding_sel: Option<&str>,
) -> anyhow::Result<()> {
    let (_, client) = connect_from_config(binding_sel)?;
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
    // --reply-to (2026-09-02, 실사용 보고): CLI 회신이 kind=reply + correlation을 실어야
    // 발신자의 wait_for_reply가 해소된다 — 본문에 "re: <id>"를 손으로 적는 우회를 없앤다
    if let Some(id) = reply_to {
        spec.kind = brevduva_protocol::Kind::Reply;
        spec.correlation_id = Some(id);
    }
    match tokio::time::timeout(Duration::from_secs(10), client.publish(spec)).await {
        Ok(Ok(id)) => println!("sent {id}"),
        Ok(Err(e)) => anyhow::bail!("rejected: {} — {}", e.code, e.message),
        Err(_) => anyhow::bail!("unconfirmed after 10s — will republish on reconnect (13.3)"),
    }
    Ok(())
}

async fn listen(binding_sel: Option<&str>) -> anyhow::Result<()> {
    let (binding, client) = connect_from_config(binding_sel)?;
    eprintln!("listening as {} — Ctrl+C to stop", binding.label());
    loop {
        if let Some(env) = client.recv(RecvFilter::Any, Duration::from_secs(60)).await {
            println!("{}", serde_json::to_string(&env)?);
        }
    }
}

async fn mcp(binding_sel: Option<&str>) -> anyhow::Result<()> {
    // 선택자 폴백: --binding → BREVDUVA_BINDING env — 데몬이 깨운 세션의 MCP가 "누가
    // 깨웠는지"를 이어받는 통로 (2026-09-02). 바인딩이 여럿인 머신에서도 깨운 세션이
    // 올바른 정체성으로 붙는다 (플래그 없는 user-scope 등록 + 다중 바인딩 = select 불가였음)
    let env_sel = std::env::var("BREVDUVA_BINDING").ok();
    let binding_sel = binding_sel.or(env_sel.as_deref());
    // lazy-JOIN: 여기서 접속하지 않는다 — 첫 도구 호출 때 McpServer가 접속 (플랩 방지)
    let (_, binding, mut opts) = options_from_config(binding_sel)?;
    // 유휴 파킹 (2026-09-01): 도구 호출이 끊긴 세션은 자리를 내려놓는다 — 방치된 대화형
    // 세션의 버퍼로 배달돼 미소비 재전달 끝에 격리되는 유실을 원천 차단. 다음 도구 호출이
    // lazy-JOIN과 같은 경로로 자리를 되찾는다
    opts.idle_park = Some(brv::client::DEFAULT_IDLE_PARK);
    tracing::info!(binding = %binding.label(), "brv mcp server on stdio");
    brv::mcp::run_stdio(opts).await
}
