# Claude Code 채널 어댑터

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

## 일반 실행 세션: v0.6.36 Monitor

평소의 `claude`에 로컬 MCP가 등록되어 있고 고유 `Monitor` 도구가 제공된다면,
“자동 수신을 활성화해줘”라는 요청으로 현재 대화에 연결한다. 에이전트가
`receiver_connect(session_kind="claude-cli", monitor_available=true)`를 호출하고,
반환된 `Monitor` 도구 호출을 그 대화에서 수행한다. 사용자가 명령을 실행하거나
Channels 옵션으로 대화를 다시 시작할 필요가 없다.

`awaiting_monitor`는 아직 활성화 완료가 아니다. 스트림 연결 후 `channel_status`의
`transport_ready=true`를 확인한다. 고정 수신 안내를 받으면 `receipt`로 외부 본문을
읽고 같은 대화에서 응답한다. Monitor 종료·MCP 종료 시 수신도 멈춘다.
Monitor 제공 여부는 Claude 버전·계정·정책에 따라 달라질 수 있으며, brv가 호스트의 기능
플래그나 권한을 변경하지 않는다. [설계·실제 TUI 시험](NATIVE_SESSION_DELIVERY.md).

Windows Claude Code 2.1.263 실제 일반 TUI + 모의 모델/WS에서 Monitor의 자동 턴 시작,
receipt, 원래 문맥 유지, 같은 접속의 ACK/reply를 확인했다. Monitor 제공 상태를 격리
fixture에 재현한 시험이며 모든 계정·GUI·OS 실기 완료를 뜻하지 않는다.

## Channels 고급 설정의 범위

아래는 기존 Channels 어댑터를 선택한 경우다. Monitor 방식의 필수 준비가 아니다.

0.6.33에 `brv mcp --claude-channel`을 추가했다. 이전 배포 0.6.32에는 없다.
Claude Code 대화형 세션이 띄운 MCP 프로세스가 수신과 발신을 맡는다. 외부에서 활성
Claude 세션을 탐색하거나 사용자가 작업 ID를 복사하지 않는다. 기존 `brv connect`는
Codex Desktop용이며 Claude 채널 시작 명령으로 바뀌지 않는다.

이미 실행 중인 일반 Claude 세션에 런타임으로 채널을 붙이는 기능은 아니다.
채널을 활성화해 시작한 세션에서 동작한다. headless `-p`, 일반 웹 채팅, Claude Desktop
Code/Cowork, VS Code 확장, 다른 OS에서의 실제 Claude 동작을 이 구현으로 보장하지 않는다.

## 로컬 개발 검증용 설정

v0.6.34부터 다음 설정 출력 명령을 제공한다.

```sh
brv mcp --config <절대경로> --binding org/agent@channel setup --runner claude
```

출력의 `mcp_json`을 시험 프로젝트의 `.mcp.json`에 반영하고 `startup_argv`에 안내된
Channels 옵션으로 시작한다. 명령은 설정 파일을 자동 변경하거나 OAuth 로그인을 실행하지 않는다.

먼저 0.6.33 이상을 설치하거나 현재 소스를 `cargo build -p brv`로 빌드한다. 테스트용 에이전트·채널을 선택하고,
그 정체성을 수신하는 daemon/다른 MCP/desktop 연결은 중지한다. 한 정체성의 수신을
동시에 여러 프로세스가 소유하도록 설정하지 않는다.

테스트 프로젝트 `.mcp.json` 예시(실행 파일과 설정 파일은 실제 절대 경로로 바꾼다):

```json
{
  "mcpServers": {
    "brevduva": {
      "command": "/absolute/path/to/brv",
      "args": ["mcp", "--claude-channel", "--config", "/absolute/path/to/config.toml", "--binding", "org/agent@channel"]
    }
  }
}
```

Windows에서는 `command`를 `D:\\...\\brv.exe` 형식으로 지정한다.
현재 Anthropic의 개발용 실행 방법:

```sh
claude --dangerously-load-development-channels server:brevduva
```

이 옵션은 특정 개발 채널의 허용 목록 검사를 예외 처리하며 사용자 확인 화면을 표시한다.
조직의 Channels 정책까지 우회하지 않는다. 공식 허용 목록 등재를 리시버가 대신할 수 없다.
설치기가 이 플래그를 자동으로 추가하거나 사용자 설정을 수정하지 않는다.

[Anthropic Channels 규격](https://code.claude.com/docs/en/channels-reference),
[Channels 사용 가이드](https://code.claude.com/docs/en/channels)를 기준으로 구현했다.
확인은 2026-09-07 기준이며, 정책과 지원 버전은 실제 시험 시 다시 확인해야 한다.

## 전달과 복구

- initialize에서 `experimental.claude/channel` capability를 선언하고 `2025-06-18`로
  협상한다. `notifications/initialized` 이후 수신 루프를 시작한다. 일반 MCP 모드는
  기존 lazy-JOIN과 대기형 도구를 유지한다.
- 채널 모드에서는 알림과 MCP 응답의 stdout을 직렬화한다. 같은 Client로 송수신하며,
  `request`는 발신 확인 뒤 반환하고 답장은 알림으로 받는다. `wait_for_message`와
  `wait_for_reply`는 도구 목록에서 제외하고 직접 호출도 거부한다.
- 공통 `delivery.rs` 저널을 사용한다. Codex의 기존 저널 경로·형식은 유지하며,
  Claude는 설정 폴더의 `claude-channel/org/agent/channel/deliveries.jsonl`을 사용한다.
- 서버 메시지는 디스크 sync 후 ACK한다. stdout 알림 전에 submitting을 저장한다.
  각 프로세스의 세션 nonce와 각 전달의 receipt_token으로 수신 확인을 구분한다.
- Claude는 알림을 읽으면 가장 먼저 `receipt(message_id, receipt_token)`를 호출한다.
  accepted는 모델이 수신 확인 도구를 호출했다는 뜻이며 처리·답장 완료가 아니다.
  `acknowledge`(방송에 대한 업무 응답), 서버 ACK(영속 인계)와도 별개다.
- 한 번에 하나의 알림만 receipt를 기다린다. 60초 내 receipt가 없으면 unknown으로
  보관하고 추가 전달을 멈춘다. 늦은 receipt는 동일 세션·동일 토큰일 때 수락한다.
  이 제한은 사용자 타이핑/승인 대기가 길 때도 적용된다.
- MCP 종료 시 수신 루프도 종료한다. 재시작하면 이전 세션의 pending/submitting/unknown을
  자동으로 새 세션에 넘기지 않는다. `channel_status`로 상태를 확인한다.
- 운영자가 이전 세션 이력을 확인한 뒤 `channel_resolve`에 message_id, action
  (`received` 또는 `retry`), note(확인 근거), confirm=true를 지정한다. retry는 확정한
  단건만 현재 세션에 다시 전달한다. 판단이 틀리면 중복 작업이 가능하므로 자동 호출하지 않는다.

## 검증 범위와 남은 확인

v0.6.34의 `receiver_session_status`는 일반 MCP 준비와 Channels 모드를 구분한다.
`channel_status.host_delivery_observed`는 현재 세션의 실제 receipt가 있어야 true이며,
이전 세션의 수동 복구를 새 세션 수신의 증거로 표시하지 않는다. Channels 모드에서는
Desktop용 `receiver_connect`/`receiver_connection`을 숨기고 직접 호출도 거부한다.

`channel_pause(paused=true)`는 모델 제출만 일시정지한다. 영속 수신은 계속하며,
`false`로 재개해도 오류·불명확 상태는 지우지 않는다. 프로세스 재시작 시 pause는 유지되지 않는다.
unknown/이전 세션의 미처리 기록으로 제출이 막힌 동안에도 새 메시지는 영속 보관한다.
수신 pump 자체가 실패하면 접속도 종료하고 오류를 표시한다. 원인을 확인한 뒤 MCP를 재시작한다.

Windows 로컬 테스트로 저널 보존·중복 방지·잘못된 receipt 거부·재시작 세션 구분·수동
복구·timeout과 늦은 receipt·stdout 동시 쓰기를 확인했다. 모의 WebSocket 서버와 MCP
호스트 통합 시험에서 수신→영속 ACK→채널 알림→receipt→동일 연결의 correlation 답장
및 hops 증가를 검증했다. 기존 Codex 회귀 테스트도 유지한다.

이 테스트는 실제 Claude 새 턴 검증이 아니다. 대화형 Claude를 이용한 허용 목록/개발
설정 통과, 유휴 새 턴과 실제 receipt 호출, 사람 입력·승인 대기·두 메시지의 실제 모델
처리 순서, 강제 종료와 OS별 실제 Claude 시험은 별도로 수행해야 한다.
저널 용량 정책도 별도 후속 범위다.

## 일반 실행 환경의 활성화 실패 수정 (v0.6.35)

일반 MCP에서 `receiver_connect(session_kind="claude-cli")`를 호출하면 Desktop 셸 명령을 반환하지 않고 `automatic_delivery=false`, `reason=claude_channels_not_enabled`를 반환한다. Channels는 호스트의 시작 설정을 필요로 하므로 일반 실행 중인 세션에서 자동 활성화가 완료됐다고 보고하지 않는다. Channels 모드의 기존 전달 경로는 유지한다.
