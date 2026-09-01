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
    /// 에이전트 연결 — 일회용 코드(--enroll, 권장) 또는 관리 API 키로. 기존 설정에는
    /// 바인딩이 **추가**된다 (같은 에이전트@채널이면 갱신)
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
    /// 바인딩(에이전트×채널) 관리 — 목록·추가·제거 (페이즈 27)
    Binding {
        #[command(subcommand)]
        action: BindingCmd,
    },
    /// 설정·서버·채널 상태 점검
    Status {
        /// 프레즌스를 조회할 바인딩 ({agent}@{channel} 또는 유일한 에이전트 이름)
        #[arg(long)]
        binding: Option<String>,
    },
    /// 이 에이전트가 참가할 수 있는 채널 목록 (토큰 기준, PROTOCOL 10.2)
    Channels {
        /// 조회할 바인딩 — 바인딩이 여럿일 때 필수
        #[arg(long)]
        binding: Option<String>,
    },
    /// 메시지 한 건 발행 (수동 테스트용)
    Send {
        #[arg(long)]
        to: String,
        #[arg(long)]
        payload: String,
        /// broadcast에 ack 수집을 요구 (11장)
        #[arg(long)]
        expects_ack: bool,
        /// 발신 바인딩 — 바인딩이 여럿일 때 필수
        #[arg(long)]
        binding: Option<String>,
    },
    /// 수신 메시지를 줄 단위 JSON으로 출력 (수동 테스트용, Ctrl+C로 종료)
    Listen {
        /// 수신 바인딩 — 바인딩이 여럿일 때 필수
        #[arg(long)]
        binding: Option<String>,
    },
    /// 로컬 MCP 서버 (stdio) — Claude Code 등 MCP 호스트용
    Mcp {
        /// 이 세션의 바인딩 — 바인딩이 여럿일 때 필수 (프로젝트별 .mcp.json에 박아두라)
        #[arg(long)]
        binding: Option<String>,
    },
    /// 상주 데몬 — 전 바인딩 동시 수신, 메시지 도착 시 세션을 깨워 처리 (config의 [wake] 필요)
    Daemon {
        /// 설정 파일 절대 경로 (미지정 시 BREVDUVA_CONFIG env → OS 기본 경로).
        /// 서비스가 아닌 실행 표면(작업 스케줄러 로그온 작업 등)에서 프로필을 고정하는 통로
        /// — 2026-09-01, 윈도우 PIN 전용 사용자의 무암호 상주 경로에서 필요 실측
        #[arg(long)]
        config: Option<String>,
        #[command(subcommand)]
        action: Option<DaemonCmd>,
    },
    /// Claude Code 훅 연동 (페이즈 17) — 턴 종료 시 전 바인딩의 대기 메시지 확인
    Hook {
        #[command(subcommand)]
        action: HookCmd,
    },
    /// 무인 모드 깨우기 설정 (페이즈 21) — 데몬이 메시지 도착 시 실행할 세션과 그 권한
    Wake {
        #[command(subcommand)]
        action: WakeCmd,
    },
}

#[derive(Subcommand)]
enum BindingCmd {
    /// 설정된 바인딩 목록 (+토큰 유무·깨우기 설정)
    List,
    /// 기존 토큰으로 바인딩 추가 — 같은 에이전트를 다른 채널에도 (grant는 서버에서 검증)
    Add {
        /// 에이전트 이름 (이미 이 머신에 토큰이 있어야 함 — 없으면 enroll 먼저)
        #[arg(long)]
        agent: String,
        /// 참가할 채널 (대시보드에서 grant 부여 후)
        #[arg(long)]
        channel: String,
        /// 능력 선언 소개문
        #[arg(long, default_value = "")]
        description: String,
    },
    /// 바인딩 제거 — 토큰은 남는다 (같은 에이전트의 다른 바인딩이 쓸 수 있음)
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
    /// 깨우기 설정 생성·수정 — 지정한 값만 바꾸고 나머지는 유지 (멱등)
    Set {
        /// 무인 세션 권한: respond(응답 전용, 기본)|edit(+파일 편집)|full(+셸).
        /// --binding과 함께면 그 바인딩 전용 인자 오버라이드, 아니면 전역
        #[arg(long)]
        allow: Option<String>,
        /// 깨우기 실행 파일 — 미지정 시 PATH에서 claude를 찾아 절대 경로로 저장.
        /// --binding과 함께면 그 바인딩 전용 러너 오버라이드(codex 등), 아니면 전역
        #[arg(long)]
        command: Option<String>,
        /// 깨어난 세션의 작업 디렉터리 — **바인딩별** (신규 단일 바인딩에서 미지정 시 현재 디렉터리)
        #[arg(long)]
        dir: Option<String>,
        /// 깨운 세션의 최대 실행 시간(초) — 항상 전역 (머신 정책)
        #[arg(long)]
        timeout: Option<u64>,
        /// always(도착 시 깨움)|never(저널 기록만) — **바인딩별**
        #[arg(long)]
        policy: Option<String>,
        /// 바인딩별 값(--dir/--policy, 그리고 오버라이드로서의 --command/--allow)의 대상
        #[arg(long)]
        binding: Option<String>,
    },
    /// 현재 깨우기 설정과 실효 명령줄 표시
    Show,
    /// 무해한 프롬프트로 깨우기를 1회 실제 실행 — 명령 경로·환경이 동작하는지 검증
    Test {
        /// 어느 바인딩의 wake_dir에서 실행할지 — 바인딩이 여럿일 때 필수
        #[arg(long)]
        binding: Option<String>,
    },
}

#[derive(Subcommand)]
enum HookCmd {
    /// ~/.claude/settings.json에 Stop 훅 등록 (멱등)
    Install,
    /// Stop 훅 진입점 — Claude Code가 호출한다 (직접 실행할 일 없음)
    #[command(hide = true)]
    Stop,
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
            binding,
        } => send(to, payload, expects_ack, binding.as_deref()).await,
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
                brv::daemon::run(cfg, tokens).await
            }
            Some(DaemonCmd::Install { config }) => brv::service::install(config.as_deref()),
            Some(DaemonCmd::Uninstall) => brv::service::uninstall(),
            // main()이 런타임 진입 전에 처리한다
            Some(DaemonCmd::ServiceRun { .. }) => unreachable!("service-run은 main에서 분기"),
        },
        Cmd::Wake { action } => match action {
            WakeCmd::Set {
                allow,
                command,
                dir,
                timeout,
                policy,
                binding,
            } => wake_set(allow, command, dir, timeout, policy, binding.as_deref()),
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

/// `brv wake set` — 전역([wake]: 실행기·권한·타임아웃)과 바인딩별(dir·policy, 그리고
/// --binding과 결합된 --command/--allow = 러너 오버라이드)을 한 명령으로.
/// 지정한 값만 갱신하고 나머지는 유지(멱등). 설정 파일이 단일 진실이라 재부팅·데몬
/// 재시작·재init(보존은 upsert가 담당)에도 계속 유지된다.
fn wake_set(
    allow: Option<String>,
    command: Option<String>,
    dir: Option<String>,
    timeout: Option<u64>,
    policy: Option<String>,
    binding_sel: Option<&str>,
) -> anyhow::Result<()> {
    let mut cfg = config::load()?; // 연결 설정(init) 위에 얹는다 — 미init이면 여기서 안내됨
    // --binding이 있으면 --command/--allow는 그 바인딩의 오버라이드 (2026-09-01 러너 혼용)
    let binding_scoped = binding_sel.is_some();
    if let Some(p) = &policy {
        anyhow::ensure!(
            p == "always" || p == "never",
            "unknown --policy {p:?} — one of: always, never"
        );
    }
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

    // ---- 바인딩부 (페이즈 27): dir·policy + 스코프된 command/allow ----
    let needs_binding = binding_scoped || dir.is_some() || policy.is_some();
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
        if let Some(p) = policy {
            target.wake_policy = p;
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
            "  {:34} policy {:6} dir {} — {runner}",
            b.full_label(),
            b.wake_policy,
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
    let prompt = "This is `brv wake test` — a harness self-check, not a real task. \
                  Print exactly `wake ok` and finish immediately. Do not call any tools.";
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
    println!("에이전트 {agent} @ 조직 {org} — 참가 가능 채널:");
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
        println!("  (없음 — 대시보드에서 채널 참가 권한을 부여하세요)");
    } else {
        println!("(* = 이 머신에 바인딩됨. 추가: brv binding add --agent {agent} --channel <ch>)");
    }
    Ok(())
}

fn token_store_note(stored: &config::TokenStore) -> String {
    match stored {
        config::TokenStore::Keyring => "토큰은 OS 키체인".to_owned(),
        config::TokenStore::File(p) => format!("키체인 없음 — 토큰 파일 {p:?} (600 권한)"),
    }
}

/// init/enroll 결과를 기존 설정에 병합 — 페이즈 27: 덮어쓰기가 아니라 바인딩 upsert.
/// 파손된 기존 설정은 정직하게 실패한다 (덮어써서 다른 바인딩을 날리지 않는다).
fn merge_binding(server: &str, binding: Binding) -> anyhow::Result<(BrvConfig, bool)> {
    let path = config::config_path()?;
    let mut cfg = if path.exists() {
        let existing = config::load()?;
        anyhow::ensure!(
            existing.server == server,
            "config already targets {} — 설정 하나는 서버 하나다. 다른 서버는 BREVDUVA_CONFIG 프로필로 분리하라",
            existing.server
        );
        existing
    } else {
        BrvConfig {
            server: server.to_owned(),
            wake: None,
            bindings: Vec::new(),
        }
    };
    let replaced = cfg.upsert_binding(binding);
    Ok((cfg, replaced))
}

/// `brv init --enroll <코드>` — 대시보드 발급 코드 하나로 연결 (페이즈 10, PROTOCOL 10.1).
async fn enroll_init(
    server: String,
    code: String,
    channel: Option<String>,
    no_mcp: bool,
) -> anyhow::Result<()> {
    let enrolled = brv::enroll::exchange(&server, code.trim(), channel.as_deref()).await?;
    let binding = enrolled.binding.clone();
    let (cfg, replaced) = merge_binding(&enrolled.server, enrolled.binding)?;
    let stored = config::store_token(&cfg.server, &binding, &enrolled.token)?;
    let path = config::store(&cfg)?;
    println!(
        "연결 완료 — 조직 {org} / 에이전트 {agent} / 채널 {ch}{note}",
        org = enrolled.org,
        agent = binding.agent,
        ch = binding.channel,
        note = if replaced {
            " (기존 바인딩 갱신 — 토큰 회전됨)"
        } else {
            ""
        },
    );
    if enrolled.channels.len() > 1 {
        println!(
            "  부여된 다른 채널: {} (추가 수신: brv binding add --agent {} --channel <ch>)",
            enrolled
                .channels
                .iter()
                .filter(|c| **c != binding.channel)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            binding.agent,
        );
    }
    println!("  설정: {path:?} / {}", token_store_note(&stored));
    if no_mcp {
        println!("에이전트에서 쓰려면: claude mcp add brevduva -- brv mcp");
    } else {
        register_mcp(&cfg);
    }
    Ok(())
}

/// Claude Code MCP 자동 등록 (기본 켬, --no-mcp로 생략) — 실패는 온보딩 실패가 아니라 안내.
/// --scope user: 기본 스코프(local)는 실행한 디렉터리에 묶여 온보딩 목적에 안 맞는다.
/// 바인딩이 여럿이면 자동 등록하지 않는다 — 전역 `brv mcp`는 바인딩 선택이 안 되므로,
/// 프로젝트별 등록(--binding 포함)을 안내한다 (페이즈 27).
fn register_mcp(cfg: &BrvConfig) {
    if cfg.bindings.len() > 1 {
        println!("바인딩이 여럿이라 MCP 전역 자동 등록을 건너뜁니다 — 프로젝트 디렉터리마다:");
        for b in &cfg.bindings {
            println!(
                "  claude mcp add brevduva -- brv mcp --binding {}",
                b.label()
            );
        }
        return;
    }
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
    // 기존 설정에는 바인딩을 upsert — 다른 바인딩과 [wake]가 보존된다 (페이즈 27).
    // 관리 키 경로는 org를 모른다(서버 default_org 소관) — 구형과 같은 org 미상 바인딩
    let new_binding = Binding {
        org: None,
        agent: agent.clone(),
        channel,
        description,
        wake_dir: None,
        wake_policy: "always".to_owned(),
        wake_command: None,
        wake_args: None,
    };
    let (cfg, replaced) = merge_binding(&base, new_binding.clone())?;
    let stored = config::store_token(&cfg.server, &new_binding, &token)?;
    let path = config::store(&cfg)?;

    println!(
        "초기화 완료{} — 설정: {path:?} ({})",
        if replaced {
            " (기존 바인딩 갱신)"
        } else {
            ""
        },
        token_store_note(&stored)
    );
    println!();
    println!("Claude Code에 연결하려면:");
    println!("  claude mcp add brevduva -- brv mcp");
    println!();
    println!("수동 확인: brv status / brv listen / brv send --to <agent> --payload \"...\"");
    Ok(())
}

/// `brv binding list` — 바인딩 목록 + 토큰 유무·깨우기 설정.
fn binding_list() -> anyhow::Result<()> {
    let cfg = config::load()?;
    println!("서버 {} — 바인딩 {}개:", cfg.server, cfg.bindings.len());
    for b in &cfg.bindings {
        let token = if config::load_token(&cfg, b).is_ok() {
            "token ok"
        } else {
            "token MISSING — enroll 필요"
        };
        println!(
            "  {:34} {token:28} wake {:6} {}",
            b.full_label(),
            b.wake_policy,
            b.wake_dir.as_deref().unwrap_or("(dir unset)")
        );
        if !b.description.is_empty() {
            println!("    {}", b.description);
        }
    }
    if cfg.bindings.is_empty() {
        println!("  (없음 — brv init --enroll <code> 로 연결하세요)");
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
            wake_policy: "always".to_owned(),
            wake_command: None,
            wake_args: None,
        });
    let token = config::load_token(&cfg, &probe).with_context(|| {
        format!("agent {agent:?}의 토큰이 이 머신에 없다 — 먼저 `brv init --enroll`로 연결하라")
    })?;
    let (org, _, granted) = brv::client::discover_channels(&cfg.server, &token).await?;
    anyhow::ensure!(
        granted.contains(&channel),
        "agent {agent:?}는 채널 {channel:?}에 grant가 없다 — 대시보드에서 부여하라 (가능: {})",
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
            wake_policy: "always".to_owned(),
            wake_command: None,
            wake_args: None,
        },
    )?;
    let path = config::store(&cfg)?;
    println!(
        "바인딩 {} — {path:?}",
        if replaced { "갱신" } else { "추가" }
    );
    println!("데몬 수신은 재시작 후 반영: brv daemon (또는 OS 서비스 재시작)");
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
    println!("바인딩 {full} 제거 — {path:?}");
    if !cfg.bindings.iter().any(|b| b.token_id() == token_id) {
        println!("  (이 에이전트의 토큰은 키체인에 남아 있다 — 회수하려면 대시보드에서 토큰 회전)");
    }
    Ok(())
}

async fn status(binding_sel: Option<&str>) -> anyhow::Result<()> {
    let cfg = config::load()?;
    println!(
        "설정: 서버 {} / 바인딩 {}개",
        cfg.server,
        cfg.bindings.len()
    );
    for b in &cfg.bindings {
        println!("  {}", b.label());
    }
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
    // 프레즌스 조회는 JOIN을 동반한다(순간 online 표시) — 전 바인딩 순회는 프레즌스 노이즈라
    // 단일 결정이 가능할 때만 (바인딩 1개 또는 --binding 지정)
    match cfg.select(binding_sel) {
        Ok(binding) => {
            let (_, client) = connect_from_config(Some(&binding.label()))?;
            match client.presence(Duration::from_secs(10)).await {
                Ok(entries) => {
                    println!("채널 {} 프레즌스:", binding.channel);
                    for e in entries {
                        println!("  {:10} {:?}", e.agent.as_str(), e.state);
                    }
                }
                Err(e) => println!("프레즌스 조회 실패: {e}"),
            }
        }
        Err(_) if !cfg.bindings.is_empty() => {
            println!("(채널 프레즌스는 --binding {{agent}}@{{channel}} 지정 시 조회)");
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

async fn send(
    to: String,
    payload: String,
    expects_ack: bool,
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
