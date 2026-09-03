// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 러너 표와 탐지 (2026-09-04, 온보딩 재설계 1 — 근거는 brevduva-server/RUNNERS.md).
//!
//! "러너"는 리시버가 메시지마다 깨우는 CLI 에이전트다. 리시버 코드에 러너 분기는 없다 —
//! 설정에 적힌 실행 파일을 띄울 뿐이다. 이 모듈은 그 설정을 **사용자가 손으로 적지 않게**
//! 하는 층이다: 어떤 러너가 깔려 있는지 찾고(탐지), 러너별로 한 턴 실행 인자·권한 대응·
//! MCP 등록 방법을 안다(프로필). 표에 없는 러너는 `--command`/`wake_args`로 손 설정한다.
//!
//! 탐지 기준 세 가지 — 이름만 믿지 않는다:
//! 1. 아는 실행 파일 이름 (`RunnerSpec::exe`)
//! 2. PATH에 있다 — 없으면 알려진 설치 폴더(npm 전역, `~/.local/bin`, 앱 번들)를 한 번 더 본다.
//!    실측(2026-09-04, 이 머신): Codex 데스크톱 앱이 번들한 CLI는 PATH에 없고
//!    `%LOCALAPPDATA%\OpenAI\Codex\bin\<해시>\codex.exe`에 있다 — 폴백이 없으면 못 찾는다
//! 3. `--version`이 5초 안에 종료 코드 0 — 이름만 같은 다른 프로그램(`agent`·`q`·`cn` 같은
//!    흔한 이름)과 깨진 링크를 거른다. 출력 형식이 문서화된 러너는 문자열까지 대조한다.
//!    로그인 여부는 여기서 보지 않는다 — 그건 `wake test`(사전 점검)의 일이다.
//!
//! 깨우기 프로필은 **실측한 러너만** `measured`다. 나머지는 문서 기준 초안으로, `wake set
//! --runner`가 경고를 붙이고 `wake show`에 "unmeasured"로 표시한다 — 문서 확인은 실측이 아니다.
//! MCP 등록은 러너에 `mcp add` 계열 명령이 있으면 그 명령으로(러너가 자기 파일을 책임진다),
//! 설정 파일만 있는 러너는 붙여 넣을 조각을 출력한다 — 형식이 제각각(JSON·TOML·YAML·JSONC·
//! crushrc)이라 brv가 사용자 파일을 직접 고치면 파손 위험이 편의보다 크다.
//!
//! 등록 서버명은 전부 `brevduva`. 깨우기 프롬프트는 서버명과 맨 도구 이름만 쓰므로 러너별
//! 접두어(`mcp__s__t`, `mcp_s_t`, `s__t`, `s_t`, `@s/t`, `t_s`, 없음)에 영향받지 않는다.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 깨우기 프로필 — 한 턴 실행 인자와 권한 수준 대응.
pub struct WakeProfile {
    /// 기본 인자 — `{prompt}` 자리에 메시지 프롬프트가 들어간다 (별도 argv 원소여야 한다).
    pub args: &'static [&'static str],
    /// 권한 수준(respond/edit/full) → 덧붙일 인자. None이면 이 러너는 실행 인자로 권한을
    /// 표현하지 않는다 — "러너 자체 설정을 따른다"고 안내한다.
    pub allow: Option<AllowFn>,
}

/// 권한 수준(respond/edit/full) → 덧붙일 인자.
pub type AllowFn = fn(&str) -> Option<Vec<&'static str>>;

/// 로컬 `brv mcp`를 러너에 등록하는 방법.
pub enum McpRegistration {
    /// 러너의 등록 명령 인자 — `{brv}`(brv 절대 경로)·`{config}`(설정 파일 절대 경로) 치환.
    Command(&'static [&'static str]),
    /// 등록 명령이 없어 사용자가 붙여 넣을 조각 — `file`은 붙일 곳, `body`는 같은 치환 규칙.
    Snippet {
        file: &'static str,
        body: &'static str,
    },
}

pub struct RunnerSpec {
    /// 프로필 식별자 — `--runner <id>`, 설정의 `runner = "<id>"`.
    pub id: &'static str,
    pub display: &'static str,
    /// 실행 파일 이름 후보 (확장자 없이). 첫 항목이 대표 이름.
    pub exe: &'static [&'static str],
    pub version_args: &'static [&'static str],
    /// `--version` 출력에 반드시 있어야 하는 문자열 — 문서화된 러너만. 흔한 이름의 오탐 방지.
    pub version_marker: Option<&'static str>,
    pub wake: Option<WakeProfile>,
    /// 깨우기 프로필이 실측됐는가 (한 턴 실행 + MCP 도구 호출까지). 문서 확인만이면 false.
    pub measured: bool,
    pub mcp: McpRegistration,
    /// 이 러너가 MCP 도구를 모델에 보여주는 이름 형식 — 표시·문서용.
    pub tool_prefix: &'static str,
    pub note: &'static str,
}

fn allow_claude(level: &str) -> Option<Vec<&'static str>> {
    Some(match level {
        "respond" => vec!["--allowedTools", "mcp__brevduva__*"],
        "edit" => vec![
            "--allowedTools",
            "mcp__brevduva__*,Read,Glob,Grep,Edit,Write",
        ],
        "full" => vec![
            "--allowedTools",
            "mcp__brevduva__*,Read,Glob,Grep,Edit,Write,Bash",
        ],
        _ => return None,
    })
}

fn allow_codex(level: &str) -> Option<Vec<&'static str>> {
    Some(match level {
        "respond" => vec!["--sandbox", "read-only"],
        "edit" => vec!["--sandbox", "workspace-write"],
        "full" => vec!["--sandbox", "danger-full-access"],
        _ => return None,
    })
}

fn allow_gemini(level: &str) -> Option<Vec<&'static str>> {
    Some(match level {
        "respond" => vec![
            "--approval-mode",
            "default",
            "--allowed-mcp-server-names",
            "brevduva",
        ],
        "edit" => vec!["--approval-mode", "auto_edit"],
        "full" => vec!["--approval-mode", "yolo"],
        _ => return None,
    })
}

fn allow_qwen(level: &str) -> Option<Vec<&'static str>> {
    Some(match level {
        "respond" => vec!["--approval-mode", "default"],
        "edit" => vec!["--approval-mode", "auto-edit"],
        "full" => vec!["--approval-mode", "yolo"],
        _ => return None,
    })
}

fn allow_cursor(level: &str) -> Option<Vec<&'static str>> {
    Some(match level {
        "respond" => vec!["--approve-mcps"],
        "edit" => vec!["--force", "--approve-mcps"],
        "full" => vec!["--force", "--sandbox", "disabled", "--approve-mcps"],
        _ => return None,
    })
}

fn allow_droid(level: &str) -> Option<Vec<&'static str>> {
    Some(match level {
        "respond" => vec![],
        "edit" => vec!["--auto", "medium"],
        "full" => vec!["--auto", "high"],
        _ => return None,
    })
}

fn allow_vibe(level: &str) -> Option<Vec<&'static str>> {
    Some(match level {
        "respond" => vec!["--agent", "plan"],
        "edit" => vec!["--agent", "accept-edits"],
        "full" => vec!["--agent", "auto-approve"],
        _ => return None,
    })
}

fn allow_copilot(level: &str) -> Option<Vec<&'static str>> {
    Some(match level {
        "respond" => vec![],
        "edit" => vec!["--allow-tool=write"],
        "full" => vec!["--allow-all-tools"],
        _ => return None,
    })
}

fn allow_continue(level: &str) -> Option<Vec<&'static str>> {
    Some(match level {
        "respond" => vec![],
        "edit" => vec!["--allow", "Write", "--allow", "Edit"],
        "full" => vec!["--allow", "*"],
        _ => return None,
    })
}

fn allow_grok(level: &str) -> Option<Vec<&'static str>> {
    Some(match level {
        "respond" | "edit" => vec![],
        "full" => vec!["--yolo"],
        _ => return None,
    })
}

const MCP_SNIPPET_JSON: &str =
    r#"{"mcpServers":{"brevduva":{"command":"{brv}","args":["mcp","--config","{config}"]}}}"#;

/// 러너 표 — RUNNERS.md 1등급(한 턴 실행 + MCP 클라이언트, 공식 문서 확인). 순서 = 탐지·표시 순서.
pub static RUNNERS: &[RunnerSpec] = &[
    RunnerSpec {
        id: "claude",
        display: "Claude Code",
        exe: &["claude"],
        version_args: &["--version"],
        version_marker: Some("Claude Code"),
        wake: Some(WakeProfile {
            args: &["-p", "{prompt}"],
            allow: Some(allow_claude),
        }),
        measured: true,
        mcp: McpRegistration::Command(&[
            "mcp",
            "add",
            "--transport",
            "stdio",
            "--scope",
            "user",
            "brevduva",
            "--",
            "{brv}",
            "mcp",
            "--config",
            "{config}",
        ]),
        tool_prefix: "mcp__brevduva__<tool>",
        note: "실행 시 --mcp-config 주입도 됨 (daemon.rs) — 등록이 없어도 깨어난 세션에 도구가 있다",
    },
    RunnerSpec {
        id: "codex",
        display: "Codex CLI",
        exe: &["codex"],
        version_args: &["--version"],
        version_marker: Some("codex-cli"),
        wake: Some(WakeProfile {
            // 실측(2026-09-04, 이 머신, codex-cli 0.153.0-alpha.5): stdin NUL·stdout 파일로도
            // 한 턴 정상 종료(exit 0). `-c mcp_servers.*` 실행 시 주입은 **안 됨** → 영구 등록 필수
            args: &["exec", "--skip-git-repo-check", "{prompt}"],
            allow: Some(allow_codex),
        }),
        measured: false, // 한 턴 실행은 실측, MCP 도구 호출(respond 수준)은 미실측
        mcp: McpRegistration::Command(&[
            "mcp", "add", "brevduva", "--", "{brv}", "mcp", "--config", "{config}",
        ]),
        tool_prefix: "mcp__brevduva__<tool>",
        note: "데스크톱 앱 번들 CLI는 PATH에 없다 — %LOCALAPPDATA%\\OpenAI\\Codex\\bin 폴백",
    },
    RunnerSpec {
        id: "openclaw",
        display: "OpenClaw",
        exe: &["openclaw"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["agent", "--local", "--message", "{prompt}"],
            allow: None,
        }),
        measured: false,
        mcp: McpRegistration::Command(&[
            "mcp",
            "add",
            "brevduva",
            "--command",
            "{brv}",
            "--arg",
            "mcp",
            "--arg",
            "--config",
            "--arg",
            "{config}",
        ]),
        tool_prefix: "brevduva__<tool>",
        note: "상주 게이트웨이 겸용 — --local 한 턴만 쓴다. 권한은 MCP 서버별 --approval 설정",
    },
    RunnerSpec {
        id: "gemini",
        display: "Gemini CLI",
        exe: &["gemini"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["-p", "{prompt}"],
            allow: Some(allow_gemini),
        }),
        measured: false,
        mcp: McpRegistration::Command(&[
            "mcp", "add", "-s", "user", "brevduva", "{brv}", "mcp", "--config", "{config}",
        ]),
        tool_prefix: "mcp_brevduva_<tool>",
        note: "",
    },
    RunnerSpec {
        id: "copilot",
        display: "GitHub Copilot CLI",
        exe: &["copilot"],
        version_args: &["version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["-p", "{prompt}", "-s"],
            allow: Some(allow_copilot),
        }),
        measured: false,
        mcp: McpRegistration::Command(&[
            "mcp",
            "add",
            "brevduva",
            "--transport",
            "stdio",
            "--",
            "{brv}",
            "mcp",
            "--config",
            "{config}",
        ]),
        tool_prefix: "(undocumented)",
        note: "MCP 도구를 --allow-tool에 적는 표기 미확인",
    },
    RunnerSpec {
        id: "cursor",
        display: "Cursor CLI",
        exe: &["agent", "cursor-agent"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["-p", "{prompt}"],
            allow: Some(allow_cursor),
        }),
        measured: false,
        mcp: McpRegistration::Snippet {
            file: "~/.cursor/mcp.json",
            body: MCP_SNIPPET_JSON,
        },
        tool_prefix: "(undocumented)",
        note: "실행 파일 이름 `agent`가 흔하다 — --version 검증 필수. --force 없으면 편집은 제안만",
    },
    RunnerSpec {
        id: "opencode",
        display: "OpenCode",
        exe: &["opencode"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["run", "--auto", "{prompt}"],
            allow: None,
        }),
        measured: false,
        mcp: McpRegistration::Snippet {
            file: "~/.config/opencode/opencode.json",
            body: r#"{"mcp":{"brevduva":{"type":"local","command":["{brv}","mcp","--config","{config}"],"enabled":true}}}"#,
        },
        tool_prefix: "brevduva_<tool>",
        note: "권한은 설정 permission.edit|bash — 실행 인자 없음",
    },
    RunnerSpec {
        id: "goose",
        display: "Goose",
        exe: &["goose"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            // 실행마다 MCP 주입 가능 (--with-extension) — 등록 없이도 깨어난 세션에 도구가 있다
            args: &[
                "run",
                "--no-session",
                "-q",
                "--with-extension",
                "{brv} mcp --config {config}",
                "-t",
                "{prompt}",
            ],
            allow: None,
        }),
        measured: false,
        mcp: McpRegistration::Snippet {
            file: "~/.config/goose/config.yaml",
            body: "extensions:\n  brevduva:\n    name: brevduva\n    type: stdio\n    cmd: {brv}\n    args: [mcp, --config, {config}]\n    enabled: true\n    timeout: 300",
        },
        tool_prefix: "brevduva__<tool>",
        note: "승인은 env GOOSE_MODE — 서비스에서는 GOOSE_DISABLE_KEYRING 필요할 수 있음",
    },
    RunnerSpec {
        id: "qwen",
        display: "Qwen Code",
        exe: &["qwen"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["-p", "{prompt}"],
            allow: Some(allow_qwen),
        }),
        measured: false,
        mcp: McpRegistration::Command(&[
            "mcp", "add", "-s", "user", "brevduva", "{brv}", "mcp", "--config", "{config}",
        ]),
        tool_prefix: "<tool> (충돌 시 brevduva__<tool>)",
        note: "",
    },
    RunnerSpec {
        id: "kiro",
        display: "Kiro CLI (구 Amazon Q)",
        exe: &["kiro-cli", "q"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["chat", "--no-interactive", "{prompt}"],
            allow: None,
        }),
        measured: false,
        mcp: McpRegistration::Snippet {
            file: "~/.kiro/settings/mcp.json",
            body: MCP_SNIPPET_JSON,
        },
        tool_prefix: "@brevduva/<tool>",
        note: "헤드리스는 KIRO_API_KEY(유료 구독) 필수. 도구 신뢰는 --trust-tools=@brevduva/*",
    },
    RunnerSpec {
        id: "amp",
        display: "Amp",
        exe: &["amp"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["-x", "{prompt}"],
            allow: None,
        }),
        measured: false,
        mcp: McpRegistration::Command(&[
            "mcp", "add", "brevduva", "--", "{brv}", "mcp", "--config", "{config}",
        ]),
        tool_prefix: "(undocumented)",
        note: "기본이 무승인 실행 — 제한은 amp.permissions 설정. 윈도우는 WSL",
    },
    RunnerSpec {
        id: "auggie",
        display: "Auggie (Augment)",
        exe: &["auggie"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["--print", "--quiet", "--allow-indexing", "{prompt}"],
            allow: None,
        }),
        measured: false,
        mcp: McpRegistration::Command(&[
            "mcp",
            "add",
            "brevduva",
            "--command",
            "{brv}",
            "--args",
            "mcp --config {config}",
        ]),
        tool_prefix: "<tool>_brevduva",
        note: "윈도우는 WSL",
    },
    RunnerSpec {
        id: "droid",
        display: "Droid (Factory)",
        exe: &["droid"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["exec", "{prompt}"],
            allow: Some(allow_droid),
        }),
        measured: false,
        mcp: McpRegistration::Command(&[
            "mcp",
            "add",
            "brevduva",
            "{brv} mcp --config {config}",
            "--type",
            "stdio",
        ]),
        tool_prefix: "(undocumented)",
        note: "",
    },
    RunnerSpec {
        id: "crush",
        display: "Crush (Charm)",
        exe: &["crush"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["run", "-q", "{prompt}"],
            allow: None,
        }),
        measured: false,
        mcp: McpRegistration::Snippet {
            file: "~/.config/crush/crushrc",
            body: "mcp add brevduva --command {brv} --args mcp --args --config --args {config}",
        },
        tool_prefix: "mcp_brevduva_<tool>",
        note: "`run`에 --yolo 없음 — 권한은 crushrc `permissions allow …`로 사전 허용",
    },
    RunnerSpec {
        id: "cline",
        display: "Cline CLI",
        exe: &["cline"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            // 비TTY에서 승인 필요 호출은 거부된다 — --yolo가 없으면 도구를 못 쓴다
            args: &["--yolo", "{prompt}"],
            allow: None,
        }),
        measured: false,
        mcp: McpRegistration::Command(&[
            "mcp", "install", "brevduva", "--", "{brv}", "mcp", "--config", "{config}",
        ]),
        tool_prefix: "(undocumented)",
        note: "",
    },
    RunnerSpec {
        id: "junie",
        display: "Junie CLI (JetBrains)",
        exe: &["junie"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["{prompt}"],
            allow: None,
        }),
        measured: false,
        mcp: McpRegistration::Snippet {
            file: "~/.junie/mcp/mcp.json",
            body: MCP_SNIPPET_JSON,
        },
        tool_prefix: "(undocumented)",
        note: "헤드리스는 JUNIE_API_KEY 필수. 비대화형은 권한 질문 없이 신뢰",
    },
    RunnerSpec {
        id: "vibe",
        display: "Mistral Vibe",
        exe: &["vibe"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["-p", "{prompt}"],
            allow: Some(allow_vibe),
        }),
        measured: false,
        mcp: McpRegistration::Snippet {
            file: "~/.vibe/config.toml",
            body: "[[mcp_servers]]\nname = \"brevduva\"\ntransport = \"stdio\"\ncommand = \"{brv}\"\nargs = [\"mcp\", \"--config\", \"{config}\"]",
        },
        tool_prefix: "brevduva_<tool>",
        note: "윈도우 비공식",
    },
    RunnerSpec {
        id: "grok",
        display: "Grok Build (xAI)",
        exe: &["grok"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["-p", "{prompt}"],
            allow: Some(allow_grok),
        }),
        measured: false,
        mcp: McpRegistration::Snippet {
            file: "~/.grok/config.toml",
            body: "[mcp_servers.brevduva]\ncommand = \"{brv}\"\nargs = [\"mcp\", \"--config\", \"{config}\"]\nenabled = true",
        },
        tool_prefix: "(undocumented)",
        note: "헤드리스는 XAI_API_KEY",
    },
    RunnerSpec {
        id: "continue",
        display: "Continue CLI",
        exe: &["cn"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["-p", "{prompt}"],
            allow: Some(allow_continue),
        }),
        measured: false,
        mcp: McpRegistration::Snippet {
            file: "~/.continue/config.yaml",
            body: "mcpServers:\n  - name: brevduva\n    type: stdio\n    command: {brv}\n    args: [mcp, --config, {config}]\n    allowHeadless: true",
        },
        tool_prefix: "(undocumented)",
        note: "실행 파일 이름 `cn`이 흔하다 — --version 검증 필수. 헤드리스는 CONTINUE_API_KEY",
    },
    RunnerSpec {
        id: "kilo",
        display: "Kilo CLI",
        exe: &["kilo"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["run", "--auto", "{prompt}"],
            allow: None,
        }),
        measured: false,
        mcp: McpRegistration::Snippet {
            file: "~/.config/kilo/kilo.jsonc",
            body: r#"{"mcp":{"brevduva":{"type":"local","command":["{brv}","mcp","--config","{config}"],"enabled":true}}}"#,
        },
        tool_prefix: "brevduva_<tool>",
        note: "헤드리스는 KILO_API_KEY",
    },
    RunnerSpec {
        id: "codebuddy",
        display: "CodeBuddy Code",
        exe: &["codebuddy", "cbc"],
        version_args: &["--version"],
        version_marker: None,
        wake: Some(WakeProfile {
            args: &["-p", "{prompt}"],
            allow: Some(allow_claude), // Claude Code와 같은 --allowedTools 체계
        }),
        measured: false,
        mcp: McpRegistration::Command(&[
            "mcp", "add", "--scope", "user", "brevduva", "--", "{brv}", "mcp", "--config",
            "{config}",
        ]),
        tool_prefix: "mcp__brevduva__<tool>",
        note: "",
    },
];

impl std::fmt::Debug for RunnerSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerSpec")
            .field("id", &self.id)
            .field("measured", &self.measured)
            .finish_non_exhaustive()
    }
}

pub fn spec(id: &str) -> Option<&'static RunnerSpec> {
    RUNNERS.iter().find(|r| r.id == id)
}

/// 실행 파일 경로에서 프로필을 역추정 — 설정에 `runner`가 없는 구형 설정용 (`wake show`).
pub fn spec_for_command(command: &str) -> Option<&'static RunnerSpec> {
    let name = Path::new(command).file_name()?.to_str()?;
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    RUNNERS
        .iter()
        .find(|r| r.exe.iter().any(|e| e.eq_ignore_ascii_case(stem)))
}

/// 프로필의 한 턴 인자 — 기본 인자 + 권한 수준. 권한을 인자로 표현하지 않는 러너는 기본 인자만.
pub fn wake_args(spec: &RunnerSpec, level: &str) -> Option<Vec<String>> {
    let profile = spec.wake.as_ref()?;
    let mut args: Vec<String> = profile.args.iter().map(|a| (*a).to_owned()).collect();
    if let Some(allow) = profile.allow {
        args.extend(allow(level)?.into_iter().map(str::to_owned));
    }
    Some(args)
}

/// 인자가 어느 권한 수준의 산출물인지 역판별 (`wake show`) — 손 편집분은 None.
pub fn level_of(spec: &RunnerSpec, args: &[String]) -> Option<&'static str> {
    ["respond", "edit", "full"]
        .into_iter()
        .find(|level| wake_args(spec, level).as_deref() == Some(args))
}

/// 탐지된 러너 — 실제로 실행되는 절대 경로와 그 버전 문자열.
#[derive(Debug, Clone)]
pub struct Detected {
    pub spec: &'static RunnerSpec,
    pub path: PathBuf,
    pub version: String,
}

/// 표의 러너를 전부 탐지한다. 순서는 표 순서. 한 러너에 후보 경로가 여럿이면(PATH와 앱 번들)
/// PATH가 우선 — 사용자가 터미널에서 치는 것과 같은 것이 깨어나야 한다.
pub fn detect_all() -> Vec<Detected> {
    RUNNERS.iter().filter_map(detect).collect()
}

pub fn detect(spec: &'static RunnerSpec) -> Option<Detected> {
    candidates(spec)
        .into_iter()
        .find_map(|path| verify(spec, &path))
}

/// PATH에서 실행 파일 탐색 (윈도우는 PATHEXT 중 실행 가능한 셋).
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| in_dir(&dir, name))
}

fn in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    exts.iter().find_map(|ext| {
        let p = dir.join(format!("{name}{ext}"));
        p.is_file().then_some(p)
    })
}

/// 후보 경로 — PATH → 공통 설치 폴더 → 러너별 앱 번들. 중복 제거.
fn candidates(spec: &RunnerSpec) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    for name in spec.exe {
        if let Some(p) = find_in_path(name) {
            push(p);
        }
        for dir in fallback_dirs() {
            if let Some(p) = in_dir(&dir, name) {
                push(p);
            }
        }
    }
    if spec.id == "codex" {
        for p in codex_app_bundle() {
            push(p);
        }
    }
    out
}

/// PATH에 빠져 있기 쉬운 설치 폴더들 — npm 전역, 사용자 로컬 bin, Homebrew, Claude 구형 설치.
fn fallback_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".npm-global").join("bin"));
        dirs.push(home.join(".claude").join("local"));
    }
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
    } else {
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
    }
    dirs
}

/// Codex 데스크톱 앱이 번들한 CLI (실측 2026-09-04): `~/.codex/config.toml`의 `CODEX_CLI_PATH`가
/// 현재 것을 가리키고, `%LOCALAPPDATA%\OpenAI\Codex\bin\<해시>\codex.exe`가 버전마다 하나씩 남는다.
/// 설정이 가리키는 것을 먼저, 없으면 가장 최근에 바뀐 폴더.
fn codex_app_bundle() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = dirs::home_dir()
        && let Ok(cfg) = std::fs::read_to_string(home.join(".codex").join("config.toml"))
    {
        for line in cfg.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("CODEX_CLI_PATH")
                && let Some((_, value)) = rest.split_once('=')
            {
                let p = PathBuf::from(value.trim().trim_matches(['\'', '"']));
                if p.is_file() {
                    out.push(p);
                }
            }
        }
    }
    if cfg!(windows)
        && let Some(local) = std::env::var_os("LOCALAPPDATA")
        && let Ok(entries) = std::fs::read_dir(
            PathBuf::from(local)
                .join("OpenAI")
                .join("Codex")
                .join("bin"),
        )
    {
        let mut versions: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let exe = e.path().join("codex.exe");
                let modified = e.metadata().ok()?.modified().ok()?;
                exe.is_file().then_some((modified, exe))
            })
            .collect();
        versions.sort_by_key(|v| std::cmp::Reverse(v.0));
        out.extend(versions.into_iter().map(|(_, p)| p));
    }
    out
}

/// 후보를 실제로 실행해 러너임을 확인한다 — 종료 코드 0, 5초 제한, 문서화된 마커 대조.
fn verify(spec: &'static RunnerSpec, path: &Path) -> Option<Detected> {
    let output = run_with_timeout(path, spec.version_args, Duration::from_secs(5))?;
    let text = output.lines().next().unwrap_or("").trim().to_owned();
    if let Some(marker) = spec.version_marker
        && !output.contains(marker)
    {
        return None; // 이름만 같은 다른 프로그램
    }
    Some(Detected {
        spec,
        path: path.to_path_buf(),
        version: text,
    })
}

/// stdout+stderr를 돌려준다. 시간 초과·실행 실패·비0 종료는 None. 표준 라이브러리만으로
/// 타임아웃을 구현한다 — `try_wait` 폴링 뒤 초과 시 kill.
fn run_with_timeout(path: &Path, args: &[&str], limit: Duration) -> Option<String> {
    let mut child = std::process::Command::new(path)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = child.wait_with_output().ok()?;
                if !status.success() {
                    return None;
                }
                let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&out.stderr));
                return Some(text);
            }
            Ok(None) if started.elapsed() < limit => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// MCP 등록 텍스트의 치환 — `{brv}`·`{config}`.
pub fn fill(template: &str, brv: &Path, config: &Path) -> String {
    template
        .replace("{brv}", &brv.to_string_lossy())
        .replace("{config}", &config.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_profile_reproduces_the_legacy_presets() {
        // 기존 설정 파일의 args와 바이트 단위로 같아야 `wake show`가 프리셋을 계속 알아본다
        let claude = spec("claude").unwrap();
        assert_eq!(
            wake_args(claude, "respond").unwrap(),
            crate::config::wake_preset_args("respond").unwrap()
        );
        assert_eq!(
            wake_args(claude, "full").unwrap(),
            crate::config::wake_preset_args("full").unwrap()
        );
        assert_eq!(
            level_of(claude, &wake_args(claude, "edit").unwrap()),
            Some("edit")
        );
        assert_eq!(level_of(claude, &["-p".to_owned()]), None);
    }

    #[test]
    fn every_profile_has_exactly_one_prompt_slot_as_its_own_argument() {
        // `{prompt}`는 별도 argv 원소여야 한다 — 다른 글자와 붙으면 치환은 되지만 인용이 깨진다
        for r in RUNNERS {
            let Some(w) = &r.wake else { continue };
            let slots = w.args.iter().filter(|a| a.contains("{prompt}")).count();
            assert_eq!(slots, 1, "{}: prompt slot count", r.id);
            assert!(
                w.args.contains(&"{prompt}"),
                "{}: prompt must be its own argument",
                r.id
            );
            for level in ["respond", "edit", "full"] {
                assert!(wake_args(r, level).is_some(), "{}: level {level}", r.id);
            }
        }
    }

    #[test]
    fn ids_and_exe_names_are_unique() {
        let mut ids: Vec<&str> = RUNNERS.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), RUNNERS.len(), "duplicate runner id");
        let mut exes: Vec<&str> = RUNNERS.iter().flat_map(|r| r.exe.iter().copied()).collect();
        exes.sort_unstable();
        exes.dedup();
        assert_eq!(
            exes.len(),
            RUNNERS.iter().map(|r| r.exe.len()).sum::<usize>(),
            "an executable name maps to two runners"
        );
    }

    #[test]
    fn command_path_maps_back_to_its_profile() {
        assert_eq!(
            spec_for_command(r"C:\Users\x\AppData\Roaming\npm\codex.cmd")
                .unwrap()
                .id,
            "codex"
        );
        assert_eq!(
            spec_for_command("/usr/local/bin/cursor-agent").unwrap().id,
            "cursor"
        );
        assert!(spec_for_command("/opt/tools/aider").is_none());
    }

    #[test]
    fn mcp_templates_substitute_both_placeholders() {
        let brv = Path::new("/usr/local/bin/brv");
        let cfg = Path::new("/home/u/.config/brevduva/config.toml");
        for r in RUNNERS {
            let text = match &r.mcp {
                McpRegistration::Command(args) => args.join(" "),
                McpRegistration::Snippet { body, .. } => (*body).to_owned(),
            };
            let filled = fill(&text, brv, cfg);
            assert!(
                !filled.contains("{brv}") && !filled.contains("{config}"),
                "{}: unfilled placeholder in {filled}",
                r.id
            );
            assert!(
                filled.contains("brevduva"),
                "{}: server must be named brevduva",
                r.id
            );
        }
    }

    #[test]
    fn version_check_rejects_a_program_that_fails_or_hangs() {
        // 종료 코드가 0이 아닌 프로그램은 러너가 아니다
        let (prog, args): (&str, &[&str]) = if cfg!(windows) {
            ("cmd", &["/d", "/c", "exit 3"])
        } else {
            ("sh", &["-c", "exit 3"])
        };
        assert!(
            run_with_timeout(&find_in_path(prog).unwrap(), args, Duration::from_secs(5)).is_none()
        );
        // 제한 시간 안에 안 끝나면 죽이고 None
        let (prog, args): (&str, &[&str]) = if cfg!(windows) {
            ("cmd", &["/d", "/c", "ping -n 30 127.0.0.1 >NUL"])
        } else {
            ("sh", &["-c", "sleep 30"])
        };
        let started = Instant::now();
        assert!(
            run_with_timeout(
                &find_in_path(prog).unwrap(),
                args,
                Duration::from_millis(300)
            )
            .is_none()
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timeout must actually cut the wait"
        );
    }
}
