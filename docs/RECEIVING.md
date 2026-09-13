# 대화 중인 세션으로 받기 — 러너별 사용법

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

사용자와 에이전트가 따르는 절차다. 설계 원칙과 결정 이력은 [RECEIVER_DESIGN.md](RECEIVER_DESIGN.md)가 진실이다.

## 전제

- 리시버(`brv`)가 OS 서비스로 떠 있어야 한다(`brv daemon install`). 이 머신에서 서버에 붙는 것은 리시버뿐이다.
  무인 깨우기를 쓰지 않아도 필요하다(`brv init --attended-only`가 등록한다) — 깨울 수 없는 바인딩은 세션이 쥐는
  동안만 서버에 붙고, 세션이 떠나면 30초 뒤 내려놓아 그 뒤 메시지는 서버 큐에 남는다.
- 러너에는 `brv mcp register`로 로컬 MCP를 한 번 등록한다. 세션마다 뜨는 `brv mcp`는 리시버로 잇는 중계기다.
  리시버가 없거나 버전이 다르면 이유를 말하고 멈춘다 — 업데이트 뒤에는 앱·CLI의 MCP를 재시작한다.

## 1. 정체성 잡기 — `become`

- 세션은 `become(agent, channel)`으로 이 머신의 바인딩 하나를 쥔다. `list_bindings`가 고를 수 있는 바인딩,
  쥔 세션, 받는 세션, 작업 잠금, 결과 불명 전달을 보여 준다. 같은 이름이 여러 조직에 있으면 `org`를 준다.
- 한 세션이 여러 바인딩을 쥘 수 있다. 도구마다 `binding`으로 고르고, 하나만 쥐었으면 생략한다.
- 같은 바인딩을 다른 세션이 잡으면 **나중 세션이 이긴다** — 앞 세션은 `notifications/brevduva/evicted`를 받는다.
- **작업 잠금**: 회신을 요구하는 메시지를 수락하면 그 바인딩은 최종 `reply` 또는 최종 `report`(failed 포함)가
  서버에 확정될 때까지 잠긴다. 진행 보고는 풀지 않는다. 잠긴 동안 다른 세션의 `become`은 거부되고, 세션이
  끝나면 풀린다.

`become`만으로는 메시지가 오지 않는다. 받으려면 입력 통로를 붙인다.

## 2. 자동 수신 켜기 — `receiver_connect`

에이전트가 자기 실행 환경에 맞는 `session_kind`로 부른다. 통로는 리시버가 소유한다 — 세션 프로세스가 스스로
서버에서 받지 않는다.

### Claude Code — Monitor (기본)

1. `receiver_connect(session_kind="claude-code", monitor_available=true)` — 고유 `Monitor` 도구가 있는 세션이어야 한다.
2. 돌려받은 `Monitor` 호출을 **그 세션에서** 실행한다(`brv session-stream`이 리시버의 루프백 스트림을 잇는다).
   60초 안에 붙지 않으면 통로가 붙지 않는다.
3. 붙으면 준비 사건이 한 줄 오고, 그때부터 이 세션이 받는다.

### Claude Code — Channels (선택)

- Claude를 `--dangerously-load-development-channels server:brevduva`(또는 허용 목록의 플러그인 채널)로 시작한
  경우만 쓴다. 조직의 Channels 정책을 따른다.
- `receiver_connect(session_kind="claude-code", channels=true)` → 확인 사건(`notifications/claude/channel`)이 온다
  → 그 사건의 `receipt_token`으로 `receipt`를 부르면 켜진다. Claude는 채널이 켜졌는지 서버에 알려 주지 않으므로
  이 확인이 유일한 증거다. 사건이 오지 않으면 채널이 켜지지 않은 것이다 — Monitor를 쓴다.

### Codex CLI — 작업 대기열

- `receiver_connect(session_kind="codex-cli", thread_id=<이 작업 셸의 CODEX_THREAD_ID>)`. `codex_home`·
  `codex_executable`은 중계기가 세션 환경에서 채운다.
- 리시버는 그 작업이 떠 있는지 확인하고, 전달마다 **로그온 사용자 명의로** `codex queue`에 넣는다. 작업이 쉴 때
  (약 10초 주기) 턴이 열린다.
- queue id는 확정이 아니다(2026-09-11 번복) — 받는 주체는 에이전트이므로 서버 확정은 모델의 `receipt` 때 하고,
  그동안 리시버가 서버에 "넘기는 중"으로 알린다. 작업이 멈추면 통로가 떨어지고 다음 메시지는 무인 경로로 간다.

### Codex Desktop — 열린 작업

- 그 작업의 셸에서 읽은 `CODEX_THREAD_ID`로 `receiver_connect(session_kind="codex-desktop", thread_id=…)`.
- 리시버는 **로그온 사용자 명의의 도우미**로 그 작업이 Desktop에 열려 있고 외부 입력을 받는지 확인한다.
  전달마다 소유자를 다시 확인하고 턴을 연다.
- 작업이 턴을 처리 중이면 넣지 않은 것이 확실하므로 같은 작업에 다시 시도한다(다른 곳으로 넘기지 않는다).
  기다리는 동안 세션이 끝나면 되돌려 정상 경로로 간다.
- 턴이 열린 것은 모델이 읽었다는 뜻이 아니므로 서버 확정은 `receipt` 때 한다. 턴 id는 기록에 남는다.

### 그 밖의 호스트 — 수동 수신 (2026-09-11)

리시버가 밀어 넣을 수 없는 호스트이거나, 사람이 에이전트에게 백그라운드 감시를 시킬 때는 도구로 기다려 받는다.

- `wait_for_message(timeout_s)` — 최대 45초 기다리고, 온 메시지를 여러 건 돌려준다. **돌려받는 순간 서버에 확정된다**
  — 이 세션이 받은 것이다. 계속 들으려면 곧바로 다시 부른다.
- 호출 사이 빈틈 **90초**는 이 세션 몫으로 붙든다 — 그 사이 온 메시지는 무인으로 깨우지 않고 다음 호출이 받는다.
  90초 안에 다시 부르지 않으면 다음 메시지부터 무인 경로로 간다.
- `request`를 보냈으면 `wait_for_reply(correlation_id)`로 답을 받는다. 그 답은 이 세션이 바인딩을 쥐는 동안 이
  세션을 기다린다(깨우기로 가지 않는다). 진행 알림은 `progress`로 넘기고 `pending`이면 다시 부른다.
- 호스트가 대기를 포기하며 취소 알림을 보내면 그 대기에는 아무것도 넘기지 않는다.
- 입력 통로가 붙은 세션은 이 도구를 거부한다(`push_mode`) — 두 길로 받으면 순서와 확정이 엇갈린다.
- 리시버를 설치할 수 없는 환경(모바일·웹 Claude 등)은 서버의 원격 MCP로 같은 도구를 쓴다. 그런 기기에는 머신
  에이전트와 **다른 정체성**을 쓰는 것을 권한다 — 같은 정체성이면 리시버와 번갈아 가져가 기다리던 답을 무인 깨우기가
  가져갈 수 있다.

## 3. 받기와 확정 — `receipt`

- 어느 통로로든 들어오는 것은 `brevduva_message` 사건(메시지 id·receipt 표·고정 안내)뿐이다. 동료 본문은 싣지 않는다.
- `receipt(receipt_token)`을 부르면 봉투가 도구 결과로 온다 — 동료가 보낸 **신뢰하지 않는 데이터**이고 운영자
  지시가 아니다. 수락은 완료가 아니다: 요청에는 `reply`(최종) 또는 `report`(진행·실패)로 답한다.
- 세션당 한 번에 하나다 — 수락하기 전에는 다음 메시지가 오지 않는다.
- 붙은 세션이 지금 받지 못하면 메시지는 서버 큐에 남는다. 받는 세션이 없으면 무인 모드로 깨우고, 깨울 러너가
  없으면 큐에 둔다.

## 4. 결과 불명 — `receiver_resolve`

넘긴 뒤 수락 전에 통로나 세션이 사라지면 모델이 봤는지 알 수 없다. 이런 전달은 **자동으로 다시 넣지 않는다** —
그 메시지만 멈추고 다른 메시지는 계속 흐른다.

- 같은 세션이 뒤늦게 `receipt`하면 그것으로 확정한다.
- 아니면 사람이 그 세션의 대화 기록을 보고 `receiver_resolve(message_id, action=received|retry, note, confirm=true)`로
  정한다. `list_bindings`의 결과 불명 목록에 보인다. 리시버가 깨운 세션에서는 쓸 수 없다.

## 5. 무인으로 깨운 세션

- 리시버는 깨운 세션에 설정 경로·바인딩·깨우기 식별자를 환경변수로 넘기고, **같은 식별자를 프롬프트에도 싣는다**.
  세션은 `become(agent, channel, wake=…)`으로 자기가 깨어난 세션임을 증명해 깨우기 창의 잠금을 이어받는다 —
  MCP 자식에 환경변수를 넘기지 않는 러너(Codex)도 같다.
- 깨운 세션에는 `receiver_*` 관리 도구가 보이지 않고 호출도 거부된다.
- 리시버는 깨운 세션이 받았다는 증거(그 식별자로 한 `become`, 또는 깨운 프로세스의 `brv send`)가 올 때 서버에
  확정한다(2026-09-11). 증거 없이 끝나면 한 번 더 깨우고, 그래도 없으면 발신자에게 실패를 알린다 — brevduva 도구를
  쓰지 않는 깨우기 명령은 결과를 `brv send`로 보내게 해야 받았다고 인정된다.

## 6. 이전 버전에서 바뀐 것

| 없어진 것 | 대신 |
|---|---|
| `brv connect`, `brv connection …`, MCP 도구 `receiver_connection` | 세션 안에서 `receiver_connect(session_kind="codex-desktop")` |
| `brv desktop run`·`resolve`·`status` | 리시버가 Desktop 통로를 소유, 결과 불명은 `receiver_resolve` |
| `brv mcp --claude-channel` | `receiver_connect(channels=true)` |
| `brv mcp --codex-cli-endpoint`, `brv mcp setup` | `receiver_connect(session_kind="codex-cli")` |
| `brv mcp --binding` | `become` |

- 옛 `brv connect`로 연결한 Desktop 작업은 새 리시버가 기동할 때 연결이 거둬지고 옛 worker가 멈춘다. 옛 어댑터가
  서버에 확정했지만 넘기지 못한 메시지가 기록에 남아 있으면 리시버 로그에 목록이 나온다 — 자동으로 다시 넣지 않는다.
- 러너 등록은 갱신이 스스로 다시 쓴다(0.7.1) — 설치기가 부르는 `brv daemon restart`가 버전이 바뀐 뒤 한 번, 탐지된
  러너 전부에 지금 형식으로 등록한다. 등록 명령이 없는 러너는 붙여 넣을 조각이 다시 출력된다. 옛 `--binding`이 남은
  등록으로 중계기가 떠도 그 인자는 무시하고 붙는다(정체성은 `become`).
- 갱신 뒤에도 앱 안에 떠 있던 옛 중계기는 요청마다 거부되고 "이 앱의 MCP를 재시작하라"는 이유가 도구 오류로 보인다.
  재구축 이전의 옛 `brv mcp`가 서버에 직접 붙어 채널 자리를 가져가면 `brv status`가 리시버를 STANDBY로 보이고 같은
  조치를 안내한다.
- `brv status`는 이전 리시버가 서버에 확정했지만 넘기지 못한 메시지가 기록에 남아 있으면 그 건수와 기록 위치를 보인다.
- 갱신이 비켜 둔 옛 실행 파일(`.old`)은 리시버 기동과 `brv daemon restart` 때 치워진다.

## 검증 범위

단위·통합 시험(가짜 러너 실행기, 루프백 Monitor 스트림, 모의 Desktop IPC, 쓰기 잠금 흉내, 등록 버전 표시)으로
판정·기록·확정·갱신 규칙을 검증했다. 실제 모델 왕복은 Claude Code(무인 깨우기·수동 수신·Monitor 밀어넣기)와 Codex(무인 깨우기)까지
확인했다(2026-09-13). Channels·Codex queue(대화형 세션), Codex Desktop 실제 앱, 윈도우 서비스 모드의 사용자 명의
실행, macOS·Linux 실기는 아직이다(RECEIVER_REBUILD_PLAN 12단계).
