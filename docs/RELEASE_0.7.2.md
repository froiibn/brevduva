# brv 0.7.2

Copyright 2026 SEIZIA (Jaeyoung Ko)

0.7.1 실기에서 발견된 갱신 절차 결함의 수정. 설계 이력은 [RECEIVER_DESIGN.md](RECEIVER_DESIGN.md) P8 ⑧(2026-09-14).

## 바뀐 것

- **Codex의 도구 승인이 갱신 뒤에도 남는다.** Codex는 MCP 도구마다 승인을 묻고 "Always allow"를 `~/.codex/config.toml`의 `[mcp_servers.brevduva.tools.<도구>]`에 적는데, 등록을 다시 쓰는 `codex mcp add`가 그 표를 통째로 지운다 — 0.7.1의 자동 재등록이 갱신마다 승인 프롬프트를 되살릴 뻔했다. 이제 등록이 이미 같은 값이면 파일을 건드리지 않고, 다시 써야 하면 승인 표를 읽어 두었다가 항목 바로 아래 되돌려 넣는다(서식·주석 보존).

## 갱신 절차

설치 한 줄뿐이다. 앱 안에 떠 있던 옛 중계기는 앱의 MCP를 재시작한다.

## 검증

Windows GNU에서 공개 202건·fmt/clippy 통과. 격리 `CODEX_HOME`으로 실제 `codex mcp add`(0.154.0) 뒤 승인 표가 제자리에 남는 것과 두 번째 실행이 파일을 건드리지 않는 것을 실측.

설치: https://brevduva.dev/install.sh 또는 https://brevduva.dev/install.ps1
