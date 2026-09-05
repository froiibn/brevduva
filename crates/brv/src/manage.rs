// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 리시버 관리를 로컬 MCP 도구로 (2026-09-04, 온보딩 재설계 4 — PLAN 2026-09-04 ⑤).
//!
//! "리시버에서 백엔드는 codex로 세팅해줘" — 사용자가 자기 러너 안에서 말로 리시버를 조작한다.
//! 도구는 **CLI를 그대로 자식 프로세스로 실행**하는 얇은 래퍼다(`receiver_configure` = `brv wake
//! set …`). 구현이 하나뿐이라 CLI와 도구가 어긋날 수 없고, 수동 명령은 그대로 남는다.
//!
//! **유인 세션 전용.** 깨어난(무인) 세션이 이 도구를 쓰면 채널의 누군가가 문장으로 이 머신의
//! 로컬 정책을 바꿀 수 있다 — `respond` 프리셋이 안전한 이유는 셸이 없어 `brv wake set`을 못
//! 치기 때문인데, 관리 도구가 `mcp__brevduva__*` 안에 들어가면 그 문이 다시 열린다. 그래서
//! 두 신호 중 하나라도 있으면 무인으로 본다: ① 데몬이 깨울 때 넘긴 표식(`BREVDUVA_BINDING`)
//! ② 데몬 상태 파일에 "깨우기 진행 중"(러너가 환경변수를 MCP 자식에 안 넘겨도 잡힌다). 무인이면
//! 도구 목록에서 빼고(존재도 모르게), 호출 시점에 다시 검사해 거부한다. 진행 중인 깨우기가
//! 있을 때 사람의 세션도 잠시 거부되는 것은 안전한 방향의 오차라 감수한다.
//!
//! 파괴적이거나 권한을 넓히는 조작(`allow full`, 바인딩 제거, 서비스 해제)은 `confirm=true`가
//! 없으면 실행하지 않고 "사람에게 확인하라"를 돌려준다 — 부드러운 장치이고, 진짜 방어는 위의
//! 유인 전용 문이다.

use serde_json::{Value, json};

/// 이 MCP 세션이 사람이 앉은 세션인가.
pub enum Attendance {
    Attended,
    /// 무인으로 판정한 이유 — 거부 메시지에 그대로 쓴다.
    Unattended(&'static str),
}

pub fn attendance() -> Attendance {
    let waking = crate::daemon::read_state()
        .map(|st| st.bindings.values().any(|b| b.waking))
        .unwrap_or(false);
    attendance_from(std::env::var_os("BREVDUVA_BINDING").is_some(), waking)
}

/// 순수 판정 — 테스트용으로 분리.
pub fn attendance_from(env_binding: bool, waking: bool) -> Attendance {
    if env_binding {
        Attendance::Unattended("this session was woken by the daemon (BREVDUVA_BINDING is set)")
    } else if waking {
        Attendance::Unattended("a woken session is running on this machine right now")
    } else {
        Attendance::Attended
    }
}

pub fn is_management_tool(name: &str) -> bool {
    name.starts_with("receiver_")
}

const ATTENDED_NOTE: &str =
    " Attended sessions only — absent and refused inside sessions the daemon woke.";

/// 도구 정의 — `brv mcp`의 tools/list가 유인 세션일 때만 붙인다.
pub fn tool_definitions() -> Vec<Value> {
    let binding = json!({ "type": "string", "description": "binding selector (agent@channel, or org/agent@channel) — required when this machine has several bindings" });
    vec![
        json!({
            "name": "receiver_status",
            "description": format!("Show this machine's receiver: profile, bindings, detected runners (path, version), daemon state, server reachability. Same as `brv status`.{ATTENDED_NOTE}"),
            "inputSchema": { "type": "object", "properties": { "binding": binding } }
        }),
        json!({
            "name": "receiver_configure",
            "description": format!("Change how this machine wakes an agent (`brv wake set`): runner (codex, claude, …), allowance level (respond|edit|full), working directory, timeout. Pass binding to change one binding only; otherwise global. Widening to `full` needs confirm=true — ask the owner first.{ATTENDED_NOTE}"),
            "inputSchema": { "type": "object", "properties": {
                "runner": { "type": "string", "description": "runner profile id from receiver_status's runner list (e.g. codex)" },
                "allow": { "type": "string", "enum": ["respond", "edit", "full"], "description": "unattended-session allowance" },
                "dir": { "type": "string", "description": "working directory for woken sessions (project root) — per binding" },
                "timeout_s": { "type": "number", "description": "max seconds a woken session may run (global)" },
                "binding": binding,
                "confirm": { "type": "boolean", "description": "required for allow=full" }
            } }
        }),
        json!({
            "name": "receiver_wake_test",
            "description": format!("Run one real wake with a harmless prompt (`brv wake test`) — proves the runner path, login and environment. Can take up to two minutes.{ATTENDED_NOTE}"),
            "inputSchema": { "type": "object", "properties": { "binding": binding } }
        }),
        json!({
            "name": "receiver_enroll",
            "description": format!("Connect an agent to this machine with an enroll code from the dashboard (`brv init --enroll`). Attended-only setup; turn on unattended receiving afterwards with receiver_configure + receiver_daemon install.{ATTENDED_NOTE}"),
            "inputSchema": { "type": "object", "properties": {
                "code": { "type": "string", "description": "one-time enroll code (brvenr_…)" },
                "server": { "type": "string", "description": "server base URL — default: this receiver's configured server" }
            }, "required": ["code"] }
        }),
        json!({
            "name": "receiver_daemon",
            "description": format!("Control the resident daemon (`brv daemon …`): install (register the OS service — on Windows an administrator prompt appears), restart, pause (needs duration, e.g. 2h), resume, uninstall (needs confirm=true).{ATTENDED_NOTE}"),
            "inputSchema": { "type": "object", "properties": {
                "action": { "type": "string", "enum": ["install", "restart", "pause", "resume", "uninstall"] },
                "duration": { "type": "string", "description": "for pause: how long (30m, 2h, …)" },
                "confirm": { "type": "boolean", "description": "required for uninstall" }
            }, "required": ["action"] }
        }),
        json!({
            "name": "receiver_binding_remove",
            "description": format!("Remove a binding from this machine (`brv binding remove`). The agent's token stays. Needs confirm=true — ask the owner first.{ATTENDED_NOTE}"),
            "inputSchema": { "type": "object", "properties": {
                "binding": binding,
                "confirm": { "type": "boolean" }
            }, "required": ["binding", "confirm"] }
        }),
        json!({
            "name": "receiver_mcp_register",
            "description": format!("Register this receiver's local MCP server in every agent runner detected on this machine (`brv mcp register`). dry_run=true only prints what would run.{ATTENDED_NOTE}"),
            "inputSchema": { "type": "object", "properties": {
                "runner": { "type": "string", "description": "only this runner" },
                "dry_run": { "type": "boolean" }
            } }
        }),
    ]
}

/// 도구 이름·인자 → CLI argv. 확인이 필요한 조작은 `confirm=true`가 없으면 Err(안내문).
pub fn argv_for(name: &str, args: &Value) -> Result<Vec<String>, String> {
    let s = |key: &str| args[key].as_str().map(str::to_owned);
    let confirmed = crate::mcp::bool_arg(args, "confirm");
    let mut argv: Vec<String> = Vec::new();
    let mut push = |a: &str| argv.push(a.to_owned());
    match name {
        "receiver_status" => {
            push("status");
            if let Some(b) = s("binding") {
                push("--binding");
                push(&b);
            }
        }
        "receiver_configure" => {
            push("wake");
            push("set");
            let mut any = false;
            if let Some(r) = s("runner") {
                push("--runner");
                push(&r);
                any = true;
            }
            if let Some(level) = s("allow") {
                if !matches!(level.as_str(), "respond" | "edit" | "full") {
                    return Err(format!(
                        "allow must be respond, edit or full (got {level:?})"
                    ));
                }
                if level == "full" && !confirmed {
                    return Err("allow=full lets woken sessions run shell commands on this machine — anyone who can message the channel can then put it to work. Ask the owner, then call again with confirm=true.".to_owned());
                }
                push("--allow");
                push(&level);
                any = true;
            }
            if let Some(d) = s("dir") {
                push("--dir");
                push(&d);
                any = true;
            }
            if let Some(t) = args["timeout_s"].as_u64() {
                push("--timeout");
                push(&t.to_string());
                any = true;
            }
            if !any {
                return Err("nothing to change — pass runner, allow, dir or timeout_s".to_owned());
            }
            if let Some(b) = s("binding") {
                push("--binding");
                push(&b);
            }
        }
        "receiver_wake_test" => {
            push("wake");
            push("test");
            if let Some(b) = s("binding") {
                push("--binding");
                push(&b);
            }
        }
        "receiver_enroll" => {
            let Some(code) = s("code") else {
                return Err("code is required".to_owned());
            };
            let server = match s("server") {
                Some(v) => v,
                None => crate::config::load()
                    .map(|c| c.server)
                    .map_err(|e| format!("server not given and no config to read it from: {e}"))?,
            };
            push("init");
            push("--server");
            push(&server);
            push("--enroll");
            push(&code);
            // 도구 경유 셋업은 유인 전제 — 무인 전환은 사람이 receiver_configure/receiver_daemon으로 따로
            push("--attended-only");
        }
        "receiver_daemon" => {
            let Some(action) = s("action") else {
                return Err("action is required".to_owned());
            };
            push("daemon");
            match action.as_str() {
                "install" | "restart" | "resume" => push(&action),
                "pause" => {
                    let Some(d) = s("duration") else {
                        return Err("pause needs duration (e.g. 2h)".to_owned());
                    };
                    push("pause");
                    push("--for");
                    push(&d);
                }
                "uninstall" => {
                    if !confirmed {
                        return Err("uninstall stops unattended receiving on this machine. Ask the owner, then call again with confirm=true.".to_owned());
                    }
                    push("uninstall");
                }
                other => return Err(format!("unknown action {other:?}")),
            }
        }
        "receiver_binding_remove" => {
            let Some(b) = s("binding") else {
                return Err("binding is required".to_owned());
            };
            if !confirmed {
                return Err("removing a binding stops this machine from receiving for that agent. Ask the owner, then call again with confirm=true.".to_owned());
            }
            push("binding");
            push("remove");
            push(&b);
        }
        "receiver_mcp_register" => {
            push("mcp");
            push("register");
            if let Some(r) = s("runner") {
                push("--runner");
                push(&r);
            }
            if crate::mcp::bool_arg(args, "dry_run") {
                push("--dry-run");
            }
        }
        other => return Err(format!("unknown management tool {other:?}")),
    }
    Ok(argv)
}

/// CLI를 자식으로 실행하고 출력을 돌려준다. 설정 프로필은 이 MCP 세션의 것을 못 박는다
/// (`BREVDUVA_CONFIG`) — 러너가 어떤 env를 넘기든 같은 리시버를 조작하게. stdin은 NUL:
/// 묻는 명령(init)은 플래그로 답을 받는다.
pub fn run_cli(argv: &[String]) -> (Value, bool) {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return (
                json!({ "status": "error", "message": format!("current exe: {e}") }),
                true,
            );
        }
    };
    let config = match crate::config::config_path() {
        Ok(p) => p,
        Err(e) => {
            return (
                json!({ "status": "error", "message": format!("config path: {e}") }),
                true,
            );
        }
    };
    let out = std::process::Command::new(&exe)
        .args(argv)
        .env("BREVDUVA_CONFIG", &config)
        .env_remove("BREVDUVA_BINDING")
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.trim().is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(err.trim_end());
            }
            let code = out.status.code().unwrap_or(-1);
            (
                json!({
                    "status": if out.status.success() { "ok" } else { "error" },
                    "command": format!("brv {}", argv.join(" ")),
                    "exit_code": code,
                    "output": text,
                }),
                !out.status.success(),
            )
        }
        Err(e) => (
            json!({ "status": "error", "command": format!("brv {}", argv.join(" ")), "message": e.to_string() }),
            true,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-04: 무인 판정은 두 신호 중 하나면 충분 — 러너가 env를 안 넘겨도 상태 파일이 잡는다.
    #[test]
    fn unattended_when_either_signal_is_present() {
        assert!(matches!(
            attendance_from(false, false),
            Attendance::Attended
        ));
        assert!(matches!(
            attendance_from(true, false),
            Attendance::Unattended(_)
        ));
        assert!(matches!(
            attendance_from(false, true),
            Attendance::Unattended(_)
        ));
        assert!(matches!(
            attendance_from(true, true),
            Attendance::Unattended(_)
        ));
    }

    #[test]
    fn tools_map_to_the_cli_one_to_one() {
        let argv = |name: &str, args: Value| argv_for(name, &args).unwrap();
        assert_eq!(argv("receiver_status", json!({})), ["status"]);
        assert_eq!(
            argv(
                "receiver_configure",
                json!({ "runner": "codex", "binding": "backend@myapp" })
            ),
            [
                "wake",
                "set",
                "--runner",
                "codex",
                "--binding",
                "backend@myapp"
            ]
        );
        assert_eq!(
            argv(
                "receiver_configure",
                json!({ "allow": "edit", "dir": "/p", "timeout_s": 300 })
            ),
            [
                "wake",
                "set",
                "--allow",
                "edit",
                "--dir",
                "/p",
                "--timeout",
                "300"
            ]
        );
        assert_eq!(argv("receiver_wake_test", json!({})), ["wake", "test"]);
        assert_eq!(
            argv(
                "receiver_enroll",
                json!({ "code": "brvenr_x", "server": "https://s" })
            ),
            [
                "init",
                "--server",
                "https://s",
                "--enroll",
                "brvenr_x",
                "--attended-only"
            ]
        );
        assert_eq!(
            argv(
                "receiver_daemon",
                json!({ "action": "pause", "duration": "2h" })
            ),
            ["daemon", "pause", "--for", "2h"]
        );
        assert_eq!(
            argv("receiver_daemon", json!({ "action": "install" })),
            ["daemon", "install"]
        );
        assert_eq!(
            argv(
                "receiver_binding_remove",
                json!({ "binding": "a@c", "confirm": true })
            ),
            ["binding", "remove", "a@c"]
        );
        assert_eq!(
            argv(
                "receiver_mcp_register",
                json!({ "runner": "codex", "dry_run": true })
            ),
            ["mcp", "register", "--runner", "codex", "--dry-run"]
        );
    }

    /// 파괴적·확장 조작은 confirm 없이는 argv가 만들어지지 않는다 — 안내문을 돌려준다.
    #[test]
    fn destructive_or_widening_calls_need_confirmation() {
        assert!(argv_for("receiver_configure", &json!({ "allow": "full" })).is_err());
        assert!(
            argv_for(
                "receiver_configure",
                &json!({ "allow": "full", "confirm": true })
            )
            .is_ok()
        );
        assert!(argv_for("receiver_daemon", &json!({ "action": "uninstall" })).is_err());
        assert!(argv_for("receiver_binding_remove", &json!({ "binding": "a@c" })).is_err());
        assert!(
            argv_for(
                "receiver_binding_remove",
                &json!({ "binding": "a@c", "confirm": false })
            )
            .is_err()
        );
        assert!(
            argv_for("receiver_configure", &json!({})).is_err(),
            "nothing to change"
        );
        assert!(
            argv_for("receiver_daemon", &json!({ "action": "pause" })).is_err(),
            "pause needs duration"
        );
        assert!(argv_for("receiver_nope", &json!({})).is_err());
    }

    #[test]
    fn every_management_tool_is_marked_attended_only() {
        for t in tool_definitions() {
            assert!(is_management_tool(t["name"].as_str().unwrap()));
            assert!(
                t["description"]
                    .as_str()
                    .unwrap()
                    .contains("Attended sessions only")
            );
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }
}
