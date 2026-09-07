# brv 0.6.34

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

Codex CLI의 기존 작업에 메시지를 자동 전달하는 실험 어댑터를 추가했습니다.
일반 MCP 도구 연결과 자동 수신 준비 상태를 구분하고, Claude Channels 설정과 진단도 보완했습니다.

- Codex TUI와 같은 로컬 app-server에 연결하고, 정확한 작업 UUID가 로드돼 있는지 확인합니다.
  `brv mcp --codex-cli-endpoint <주소>`로 준비한 뒤 현재 TUI에서 `receiver_connect`를 사용합니다.
- 외부 메시지를 `turn/start.toolOutput`으로 전달합니다. 새 작업·headless 세션을 만들지 않으며,
  모델·작업 폴더·샌드박스·승인 설정을 덮어쓰지 않습니다.
- MCP와 자동 수신이 한 Brevduva 접속을 공유합니다. 영속 저장 후 ACK하고,
  제출·receipt·업무 완료를 구분합니다. 불명확한 전달은 자동 재제출하지 않습니다.
- `receiver_session_status`, `channel_pause`, 모드별 도구 안내와 설정 출력 명령을 추가했습니다.
  `brv mcp --config <절대경로> --binding org/agent@channel setup --runner claude` 또는
  `setup --runner codex --endpoint ws://127.0.0.1:4500`을 사용합니다.
- 영속 인계의 ACK·복구 책임을 프로토콜 원문과 영문판에 명시했습니다. wire 스키마는 유지합니다.

## 업데이트와 시작 조건

설치 후 `brv --version`이 0.6.34인지 확인하고 앱의 로컬 MCP를 재시작하세요.
**설치만으로 일반 Codex CLI가 자동 수신 모드로 바뀌지는 않습니다.**
Codex는 `codex app-server --listen <주소>`와 `codex --remote <같은-주소>`를 사용합니다.
이미 독립 실행한 일반 TUI에 임의로 붙는 기능은 아닙니다. `brv connect`는 계속 Desktop용입니다.

Claude CLI는 기존 `--claude-channel`과 Claude의 Channels 시작 옵션을 사용합니다.
개발 채널 확인과 조직 정책은 그대로 적용됩니다. 일반 MCP 등록만으로 Channels가 활성화되지 않습니다.

[Codex CLI 준비·복구](https://github.com/froiibn/brevduva/blob/v0.6.34/docs/CODEX_CLI.md) ·
[Claude Channels](https://github.com/froiibn/brevduva/blob/v0.6.34/docs/CLAUDE_CHANNEL.md)

## 검증 범위

Windows·macOS·Linux의 fmt·clippy·회귀 CI를 릴리스 커밋에서 확인한 뒤 게시합니다.
Windows 로컬 회귀 105개 통과. 실제 Windows Codex TUI 0.153.4와 로컬 모의 모델·메시지 서버로
동일 작업의 자동 턴, 문맥 보존, 화면 출력, 영속 ACK 및 단일 접속의 receipt·답신을 확인했습니다.

macOS·Linux의 빌드·회귀 통과는 해당 OS의 실제 TUI 모델 대화 검증을 의미하지 않습니다.
실제 Claude↔Codex 모델 왕복, 승인·취소 실기, macOS·Linux TUI 실기는 별도 확인 대상입니다.

설치: macOS/Linux `curl -fsSL https://brevduva.dev/install.sh | sh`,
Windows `irm https://brevduva.dev/install.ps1 | iex`.
