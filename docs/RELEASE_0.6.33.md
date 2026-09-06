# brv 0.6.33

Claude Code 대화형 세션용 채널 어댑터를 추가했습니다. 기존 Codex Desktop 연결 기능은 유지합니다.

- `brv mcp --claude-channel`: Claude 세션이 소유한 MCP 프로세스에서 메시지를 받고 알림을 보냅니다.
- `receipt`: 현재 세션의 수신 확인을 기록합니다. 메시지 처리 완료와는 구별합니다.
- `channel_status`, `channel_resolve`: 불명확한 전달을 조회하고 운영자가 이력을 확인한 뒤 복구합니다.
- Codex와 Claude가 같은 영속 전달 저널 구현을 사용하며 저장 위치는 분리합니다.
- 채널 모드에서는 수신 알림과 답장이 한 연결을 사용합니다. 대기형 수신 도구와의 경쟁을 방지합니다.

## 새 세션 테스트 전 설정

설치 후 `brv --version`이 0.6.33인지 확인하고 기존 로컬 MCP 프로세스를 재시작하세요.
두 에이전트는 같은 채널에서 **서로 다른 에이전트 이름과 키**를 사용해야 합니다.

**Codex Desktop:** 새 작업에서 로컬 Brevduva MCP를 사용하고 해당 Codex 바인딩으로 연결을 요청합니다.
예: “Brevduva의 codex@channel에 이 작업을 연결해줘.” 호스트 작업 셸의 식별자로 연결됩니다.

**Claude Code 대화형 CLI:** MCP 실행 인자에 `mcp --claude-channel --binding claude@channel`을 지정하고,
개발 검증 시 `claude --dangerously-load-development-channels server:brevduva`로 시작합니다.
`server:brevduva`는 MCP 설정 이름과 같아야 합니다. 개발 채널 확인 화면과 조직 정책을 따릅니다.
일반 설치·자동 MCP 등록만으로 이 채널 모드가 활성화되지는 않습니다.

[상세 설정·복구 절차](https://github.com/froiibn/brevduva/blob/v0.6.33/docs/CLAUDE_CHANNEL.md)

## 검증 범위

로컬 Windows 회귀 테스트 95개와 fmt·clippy 통과. 모의 서버/MCP 호스트에서
수신→영속 ACK→채널 알림→receipt→원본 correlation 답장을 검증했습니다.
실제 Claude 새 턴·receipt·답장은 사용자가 새 세션에서 검증할 대상입니다.
Claude 일반 GUI, headless `-p`, 임의 CLI 기존 세션 연결이 모두 지원된다는 의미는 아닙니다.
Anthropic의 채널 허용 목록·조직 정책은 별도 제약입니다.

설치: macOS/Linux `curl -fsSL https://brevduva.dev/install.sh | sh`,
Windows `irm https://brevduva.dev/install.ps1 | iex`.
