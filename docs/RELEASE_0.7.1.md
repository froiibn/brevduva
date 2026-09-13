# brv 0.7.1

Copyright 2026 SEIZIA (Jaeyoung Ko)

0.7.0 갱신 직후 발견된 갱신 절차 결함의 근본 수정. 설계 이력은 [RECEIVER_DESIGN.md](RECEIVER_DESIGN.md) P8 보강(2026-09-13).

## 바뀐 것

- **갱신이 러너 등록을 다시 쓴다.** 설치기가 부르는 `brv daemon restart`가 설정 옆 `mcp-registered.version`이 지금 버전이 아니면 탐지된 러너 전부에 로컬 MCP를 다시 등록한다(Claude Code는 지우고 등록, Codex는 `mcp add`가 덮어씀). 등록 명령이 없는 러너는 붙여 넣을 조각이 다시 출력된다. 이 뒤처리(옛 실행 파일 정리 포함)는 OS 서비스가 없어도 돈다.
- **옛 `--binding` 등록으로 떠도 붙는다.** 0.7.0은 이유를 말하고 종료했지만 Codex는 그 stderr를 보여 주지 않아 "MCP startup failed: … initialize response"만 남았다. 이제 그 인자는 경고만 남기고 무시한다 — 정체성은 `become`이 정한다.
- 자동 등록의 "바인딩이 여럿이면 프로젝트별 `--binding` 등록 안내" 가지를 삭제했다 — `brv mcp`는 바인딩을 고르지 않는다.

## 갱신 절차

설치 한 줄뿐이다. 앱 안에 떠 있던 옛 중계기는 앱의 MCP를 재시작한다(설치기 마지막 안내).

## 검증

Windows GNU에서 공개 199건·fmt/clippy 통과. 격리 `CODEX_HOME`으로 옛 `--binding` 항목이 새 등록으로 덮이는 것, 표시 파일 기록, 옛 인자로 뜬 중계기가 붙는 것을 실측.

설치: https://brevduva.dev/install.sh 또는 https://brevduva.dev/install.ps1
