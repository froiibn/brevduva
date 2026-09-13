# 2026-09-08 테스트 재개 기록

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

기록일: 2026-09-07. 사용자가 늦은 시간으로 실기 테스트를 내일로 연기했다.
오늘 추가 실기 메시지 전송이나 세션 재시작은 하지 않는다.

## 목표와 현재 상태

Brevduva는 벤더 독립적인 에이전트 통신망이다. 사용자가 메시지 확인을 지시하지 않아도
현재 연결한 작업이 메시지를 받아 기존 문맥에서 이어서 일하는 것이 목표다.
새 작업을 생성하는 것으로 기존 작업 깨우기를 대체하지 않는다.

- 배포 바이너리: **v0.6.33**, 커밋 `44e090d`.
- main 최신: `bc1b7c7` (설치 마지막에 매 업데이트 후 로컬 MCP 재시작 안내).
- v0.6.33 CI 성공: https://github.com/froiibn/brevduva/actions/runs/34051720273
- v0.6.33 릴리스 성공: https://github.com/froiibn/brevduva/actions/runs/34051982717
- 설치 안내 커밋 CI 성공: https://github.com/froiibn/brevduva/actions/runs/34053007248
- Windows/macOS/Linux용 5개 바이너리와 SHA256SUMS 게시 완료.
- 웹 install.sh/install.ps1 갱신 완료. 내려받은 파일과 로컬 SHA256 일치 확인.
- Windows 로컬 회귀 테스트 95개와 fmt/clippy, 설치기 문법 검사 통과.
- CI·모의 전송 검증은 실제 Claude/Codex 모델 대화 검증을 대신하지 않는다.

## 내일 시작 순서

1. 각 테스트 머신에서 웹 설치 명령을 다시 실행하고 `brv --version`이 0.6.33인지 확인한다.
   기존 안내는 README의 설치 절차를 따른다.
2. 실행 중인 AI 앱의 로컬 MCP를 재시작하거나 작업을 저장하고 앱을 완전히 종료·재실행한다.
   CLI는 동일 MCP/Channels 옵션으로 다시 실행한다. 새 대화만으로는 MCP가 재시작되지 않을 수 있다.
   꺼져 있던 앱은 시작하면 된다. 설치기는 데몬·저장된 활성 작업 worker는 재시작하지만
   앱이 소유한 MCP는 자동 재시작하지 않는다.
3. 우선 Windows의 새 Claude CLI 대화형 세션과 새 Codex Desktop 작업으로 왕복을 검증한다.
   두 에이전트는 서로 다른 키/바인딩을 사용한다. 예전 PoC 수신기와 중복 수신하지 않게 한다.
4. 수신자에게 사람이 추가 입력하지 않은 채 메시지가 새 턴을 시작하는지, 이전 문맥이 유지되는지,
   상대방에게 답장이 실제 도착하는지 확인한다. 전송 ACK나 입력 accepted만으로 성공 처리하지 않는다.
5. 이후 나머지 OS/GUI/CLI 조합을 확인한다. 앱·버전·실행 옵션·메시지 ID·실제 결과를 기록한다.
   해당 OS의 앱 또는 연결 방식이 없으면 미지원/시험 불가로 구분하며 통과로 기록하지 않는다.

### 이 Windows 머신의 준비 정보 (로컬 기록)

- 설정 파일: `C:\brevduva\config.toml` (토큰을 기록/공유하지 않는다).
- Codex 시험 바인딩: `personal/brvcodex@brv`.
- Claude 시험 바인딩: `personal/brvclaude@brv`.
- 준비된 MCP 설정: `target/claude-channel-test.mcp.json` (gitignored, 삭제됐다면 재생성 필요).
- 설정의 실행 파일은 사용자 설치 경로 `C:\Users\Jaeyoung\.local\bin\brv.exe`이고,
  인자는 `mcp --claude-channel --config C:\brevduva\config.toml --binding personal/brvclaude@brv`이다.

```powershell
claude --strict-mcp-config --mcp-config "D:\Brevduva\brevduva\target\claude-channel-test.mcp.json" --dangerously-load-development-channels server:brevduva
```

Claude의 개발 채널 확인 화면은 사용자가 승인한다. 조직 정책은 우회하지 않는다.
공식 배포에는 Anthropic 허용 목록 제약이 있다. 대화형 CLI 시험이며 `-p` 지원의 근거가 아니다.
Codex에서는 새 작업에 현재 작업을 시험 바인딩으로 연결하도록 요청한다.

## 12개 조합의 실기 상태

| OS | Codex GUI | Codex CLI | Claude GUI | Claude CLI |
|---|---|---|---|---|
| Windows | 기존 실기 통과; 0.6.33 새 세션 재검증 예정 | 미검증; 임의 라이브 CLI attach 지원 아님 | 미검증 | 0.6.33 대화형 Channels 시험 예정 |
| macOS | 미검증 | 미검증; 임의 라이브 CLI attach 지원 아님 | 미검증 | 미검증 |
| Linux | 미검증; 호환 앱/IPC 존재 확인 필요 | 미검증; 임의 라이브 CLI attach 지원 아님 | 미검증; 앱 제공 여부 확인 필요 | 미검증 |

## 구현 경계와 복구

- Codex: 정확한 Desktop 작업 소유자를 찾아 IPC로 외부 입력을 전달한다.
- Claude: 세션이 소유한 `brv mcp --claude-channel` 프로세스가 알림을 전달한다.
  `receipt`는 알림 수신 확인이며 업무 완료가 아니다.
- 영속 저널에 저장 후 서버 ACK. submitting/unknown은 자동 재전송하지 않는다.
  Claude에서 60초 동안 receipt가 없으면 unknown으로 멈추므로 실제 상태를 확인한다.
- 불명확한 전달은 저널을 지우지 말고 이력을 대조한 후 정식 수동 복구를 사용한다.
- 제품 절차: [Desktop](DESKTOP_RECEIVER.md), [Claude Channels](CLAUDE_CHANNEL.md),
  [플랫폼](PLATFORM_CONNECTION.md), [0.6.33 변경](RELEASE_0.6.33.md).
- 웹 Docs 구축은 후순위. 내일 우선 작업은 실기 검증이다.

## 오늘 확인한 미커밋 파일

제품 추적 파일에는 미커밋 수정이 없었다. 아래 10개는 모두 Git 미추적 파일이다.
이 기록 자체도 새 로컬 파일이며, 이번 기록 작업에서 커밋·배포하지 않는다.

| 파일 | 용도 및 주의 |
|---|---|
| `docs/DESKTOP_BRIDGE_POC.md` | Windows 기존 작업 전달·자율 왕복 PoC 기록. 제품 사용 설명과 구분 |
| `docs/SESSION_DELIVERY.md` | 초기 세션 전달 조사/설계안. 채택되지 않은 확장안 포함 |
| `docs/RELEASE_READINESS.md` | 과거 배포 준비 점검 누적 기록. 0.6.29~0.6.32 상태가 혼재; 최신 상태는 이 문서 기준 |
| `scripts/poc_brv_desktop_bridge.cjs` | listen→Desktop 단건 전달 실험. 운영용 영속 인계를 대체하지 않음 |
| `scripts/poc_brv_desktop_rounds.cjs` | 순차 3회 전달 실험 |
| `scripts/poc_brv_desktop_rounds.test.cjs` | 순차 실험의 테스트 |
| `scripts/poc_codex_desktop.cjs` | Windows Desktop IPC 전달 실험 |
| `scripts/poc_codex_desktop.test.cjs` | Desktop 실험의 테스트 |
| `scripts/poc_claude_channel.py` | 격리 Claude 채널 프로브; 실제 제품 통과 근거로 해석하지 않음 |
| `scripts/probe_codex_session_delivery.py` | 로컬 모의 모델을 이용한 app-server 세션 재개/전달 조사 |

초기 조사 파일은 공개 전 오래된 결론·실험 식별자·저작권 고지 등을 별도로 정리해야 한다.
임의 삭제나 일괄 커밋하지 않았다. `AGENTS.md`는 사용자 요청대로 gitignore 적용 상태다.
