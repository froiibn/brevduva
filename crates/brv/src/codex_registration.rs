// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! Codex 등록 파일(`$CODEX_HOME/config.toml`)의 `brevduva` 항목 보존 (2026-09-14, 수칙 9).
//!
//! `codex mcp add`는 같은 이름 항목을 **하위 표까지 통째로** 덮어쓴다(0.153.4·0.154.0 실측) — 사용자가
//! "Always allow"로 쌓은 `[mcp_servers.brevduva.tools.<도구>] approval_mode = "approve"`가 갱신마다 사라져
//! 승인 프롬프트가 되살아난다. 그래서 갱신이 등록을 다시 쓸 때 ① 등록 내용이 이미 지금 값이면 건드리지
//! 않고 ② 다시 써야 하면 `tools` 표를 읽어 두었다가 `mcp add` 뒤 그대로 되돌려 넣는다. 파일은 `toml_edit`로
//! 서식·주석을 보존해 고친다 — brv가 러너 설정 파일을 직접 고치는 유일한 예외이며, 이 항목 아래만 만진다.

use std::path::{Path, PathBuf};

const SERVER: &str = "brevduva";

/// Codex 설정 파일 — `CODEX_HOME`이 있으면 그 아래, 없으면 `~/.codex/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    let home = match std::env::var_os("CODEX_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => dirs::home_dir()?.join(".codex"),
    };
    Some(home.join("config.toml"))
}

/// 등록 명령의 채워진 인자(`mcp add brevduva -- <brv> mcp --config … --host codex`)에서 러너에 적힐
/// `command`·`args`를 뽑는다 — `--` 뒤가 실행 명령이다.
fn registered_command(filled: &[String]) -> Option<(&str, &[String])> {
    let at = filled.iter().position(|a| a == "--")?;
    let cmd = filled.get(at + 1)?;
    Some((cmd, &filled[at + 2..]))
}

/// `brevduva` 항목이 이미 이 명령·인자로 등록돼 있는가 — 그러면 다시 쓰지 않는다(승인 표 보존의 첫째 길).
pub fn is_current(config: &Path, filled: &[String]) -> bool {
    let Ok(text) = std::fs::read_to_string(config) else {
        return false;
    };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return false;
    };
    entry_is_current(&doc, filled)
}

fn entry_is_current(doc: &toml_edit::DocumentMut, filled: &[String]) -> bool {
    let Some((cmd, args)) = registered_command(filled) else {
        return false;
    };
    let Some(entry) = doc.get("mcp_servers").and_then(|s| s.get(SERVER)) else {
        return false;
    };
    let same_cmd = entry.get("command").and_then(|c| c.as_str()) == Some(cmd);
    let same_args = entry
        .get("args")
        .and_then(|a| a.as_array())
        .is_some_and(|arr| {
            arr.len() == args.len()
                && arr
                    .iter()
                    .zip(args)
                    .all(|(have, want)| have.as_str() == Some(want.as_str()))
        });
    same_cmd && same_args
}

/// 사용자가 쌓은 도구 승인 표(`mcp_servers.brevduva.tools`) — 다시 쓰기 전에 읽어 둔다. 없으면 None.
pub fn saved_tool_approvals(config: &Path) -> Option<toml_edit::Item> {
    let text = std::fs::read_to_string(config).ok()?;
    let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
    let tools = doc.get("mcp_servers")?.get(SERVER)?.get("tools")?;
    tools.is_table_like().then(|| tools.clone())
}

/// `codex mcp add`가 지운 승인 표를 새 항목 아래 되돌려 넣는다. 항목이 없으면(등록 실패) 아무것도 하지 않는다.
pub fn restore_tool_approvals(config: &Path, tools: toml_edit::Item) -> std::io::Result<()> {
    let text = std::fs::read_to_string(config)?;
    let mut doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    // 읽어 둔 표는 옛 파일에서의 자리(position)를 기억한다 — 그대로 넣으면 `[mcp_servers.notion]` 뒤 같은 엉뚱한
    // 곳에 흩어진다(실측). 새 항목과 같은 자리를 주면 안정 정렬이 항목 바로 뒤, 다음 항목 앞에 둔다
    let anchor = doc
        .get("mcp_servers")
        .and_then(|s| s.get(SERVER))
        .and_then(toml_edit::Item::as_table)
        .and_then(toml_edit::Table::position);
    let Some(entry) = doc
        .get_mut("mcp_servers")
        .and_then(|s| s.get_mut(SERVER))
        .and_then(|e| e.as_table_like_mut())
    else {
        return Ok(());
    };
    if entry.get("tools").is_some() {
        return Ok(()); // 러너가 이미 남겨 두었다 — 덮지 않는다
    }
    let mut tools = tools;
    place_at(&mut tools, anchor);
    entry.insert("tools", tools);
    std::fs::write(config, doc.to_string())
}

/// 표와 그 아래 모든 표에 같은 자리를 준다.
fn place_at(item: &mut toml_edit::Item, position: Option<isize>) {
    if let Some(table) = item.as_table_mut() {
        table.set_position(position);
        for (_, child) in table.iter_mut() {
            place_at(child, position);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(brv: &str) -> Vec<String> {
        [
            "mcp",
            "add",
            "brevduva",
            "--",
            brv,
            "mcp",
            "--config",
            "C:\\brevduva\\config.toml",
            "--host",
            "codex",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
    }

    const REGISTERED: &str = r#"model = "gpt-5"

[mcp_servers.brevduva]
command = 'C:\Users\me\.local\bin\brv.exe'
args = ["mcp", "--config", 'C:\brevduva\config.toml', "--host", "codex"]

[mcp_servers.brevduva.tools.receipt]
approval_mode = "approve"

[mcp_servers.brevduva.tools.reply]
approval_mode = "approve"

[mcp_servers.notion]
url = "https://mcp.notion.com/mcp"
"#;

    /// 2026-09-14 (수칙 9): 등록이 이미 지금 값이면 다시 쓰지 않는다 — 실행 파일이나 인자가 하나라도 다르면 다시 쓴다.
    #[test]
    fn a_registration_that_already_matches_is_left_alone() {
        let doc = REGISTERED.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(entry_is_current(
            &doc,
            &filled(r"C:\Users\me\.local\bin\brv.exe")
        ));
        assert!(!entry_is_current(&doc, &filled(r"C:\other\brv.exe")));
        let mut stale = filled(r"C:\Users\me\.local\bin\brv.exe");
        stale.push("--binding".into());
        assert!(!entry_is_current(&doc, &stale));
        let empty = "model = \"gpt-5\"\n"
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert!(!entry_is_current(
            &empty,
            &filled(r"C:\Users\me\.local\bin\brv.exe")
        ));
    }

    /// 2026-09-14 (수칙 9): `codex mcp add`가 항목을 통째로 덮어써도 사용자의 "Always allow" 표는 갱신 뒤 그대로다 —
    /// 다른 항목·주석·서식은 건드리지 않는다.
    #[test]
    fn tool_approvals_survive_a_rewrite_of_the_codex_entry() {
        let dir = std::env::temp_dir().join(format!("brv-codex-approvals-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        std::fs::write(&config, REGISTERED).unwrap();
        let saved = saved_tool_approvals(&config).expect("two approvals were saved");

        // `codex mcp add`의 결과를 흉내 낸다: 항목이 새 값으로 바뀌고 하위 표는 사라진다 (0.154.0 실측)
        let overwritten = r#"# my codex settings
model = "gpt-5"

[mcp_servers.brevduva]
command = 'C:\new\brv.exe'
args = ["mcp", "--config", 'C:\brevduva\config.toml', "--host", "codex"]

[mcp_servers.notion]
url = "https://mcp.notion.com/mcp"
"#;
        std::fs::write(&config, overwritten).unwrap();
        restore_tool_approvals(&config, saved).unwrap();

        let after = std::fs::read_to_string(&config).unwrap();
        let doc = after.parse::<toml_edit::DocumentMut>().unwrap();
        let tools = &doc["mcp_servers"]["brevduva"]["tools"];
        assert_eq!(tools["receipt"]["approval_mode"].as_str(), Some("approve"));
        assert_eq!(tools["reply"]["approval_mode"].as_str(), Some("approve"));
        assert_eq!(
            doc["mcp_servers"]["brevduva"]["command"].as_str(),
            Some(r"C:\new\brv.exe"),
            "the new registration stays"
        );
        assert!(
            after.starts_with("# my codex settings"),
            "comments are kept"
        );
        let notion = after.find("[mcp_servers.notion]").unwrap();
        assert!(
            after.find("[mcp_servers.brevduva.tools.receipt]").unwrap() < notion
                && after.find("[mcp_servers.brevduva.tools.reply]").unwrap() < notion,
            "the approvals sit under their entry, not scattered after other servers:\n{after}"
        );
        assert!(saved_tool_approvals(&config).is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 승인 표가 없던 파일에서는 읽을 것도 되돌릴 것도 없다.
    #[test]
    fn nothing_is_saved_when_no_approvals_exist() {
        let dir =
            std::env::temp_dir().join(format!("brv-codex-noapprovals-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        std::fs::write(
            &config,
            "[mcp_servers.brevduva]\ncommand = 'x'\nargs = [\"mcp\"]\n",
        )
        .unwrap();
        assert!(saved_tool_approvals(&config).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
