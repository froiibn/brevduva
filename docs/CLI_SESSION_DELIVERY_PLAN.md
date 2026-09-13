# CLI 기존 세션 자동 전달 확인 및 구현 계획

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

작성일: 2026-09-07. 상태: 아래 계획에 따른 실험 구현 반영. 실제 모델 왕복·출시는 미완료다.
개발 결과와 남은 검증은 문서 끝의 후속 기록을 우선한다.
사용자의 시험 세션에 메시지를 보내거나 설정·연결·실행체를 변경하지 않고 조사했다.

## 목표와 현재 판정

사람이 보고 있는 CLI 대화가 유휴 상태에서도 동료 메시지를 받고, 추가 사용자 입력 없이
같은 대화·문맥에서 처리하고 답신한다. MCP 등록 성공, 서버 ACK, 새 headless 작업의
응답은 이 목표의 성공 판정이 아니다.

| 경로 | 제품 코드 | 실기 판정 |
|---|---|---|
| 일반 `brv mcp` | 도구 호출에 의한 송수신 구현 | 유휴 CLI의 자동 턴 시작 기능은 이 경로에 없음 |
| Codex CLI 기존 대화 자동 전달 | 개발 소스에 공유 app-server 어댑터 추가 | Windows 실제 TUI + 모의 모델 통합 통과; 일반 독립 TUI attach는 미지원 |
| Claude CLI Channels | v0.6.33에 구현 | 모의 호스트 통합 시험 존재; 실제 Claude의 자동 수신·왕복은 미검증 |
| Claude Stop 훅 | 턴 종료 시 peek 후 처리 요청 구현 | 이미 유휴인 세션을 외부에서 깨우는 경로가 아님 |
| Codex Desktop | 기존 작업 전달 구현 | Windows 실기 기록 존재; Codex CLI 지원으로 해석하지 않음 |
| daemon의 Codex/Claude 실행 | 새 headless 실행체 생성 구현 | 현재 대화 자동 전달과 별도 기능 |

Claude CLI 전체가 미구현인 것은 아니다. **Channels를 켜고 시작한 대화형 Claude**와
일반 MCP만 등록한 Claude를 구분해야 한다. 현재 사용자 Claude 세션이 어떤 옵션으로
시작됐는지는 이번 조사에서 확인하지 않았다. `-p` 및 임의 실행 중 세션 attach는 별도 범위다.

## 코드 근거

- `crates/brv/src/mcp.rs:67`: stdio 요청 처리 중 별도 pump는 `self.channel`이 있을 때만 시작한다.
  `with_channel`은 idle park를 해제한다. initialize의 Channels capability와
  `notifications/initialized` 후 pump 시작이 실제로 연결돼 있다.
- `crates/brv/src/mcp.rs:149`: Channels에서는 `wait_for_message`/`wait_for_reply`를 제외하고
  receipt/status/resolve를 제공한다. 일반 MCP는 `ensure_client`와 수신 도구 호출 경로다.
- `crates/brv/src/claude_channel.rs:63`: submitting 저장 후
  `notifications/claude/channel` 알림을 구성한다. 세션별 receipt_token을 검증한다.
- `crates/brv/src/claude_channel.rs:207`: `recv_manual` → 저널 저장 → `confirm` → stdout 알림을 실행한다.
  60초 receipt 부재는 unknown으로 보존하고 추가 전달을 막으며 늦은 동일 receipt를 허용한다.
- `crates/brv/src/mcp.rs:700` 이후 통합 시험은 모의 WS·MCP 호스트를 사용한다.
  실제 Claude 모델 새 턴의 증거는 아니다.
- `crates/brv/src/connection.rs:25,321`: adapter가 `codex-desktop`이며 실제 Desktop owner probe를 요구한다.
  `CODEX_THREAD_ID` 존재만으로 CLI 지원을 선언하지 않는다.
- `crates/brv/src/hook.rs`: Stop 실행 시 peek하고 block을 반환한다. 유휴 중 push listener가 아니다.
- `crates/brv/src/runners.rs:46`: 과거의 Passive/TurnEndHook 분류와 Direct 관련 주석은
  현재 Desktop·Channels 구현 전체를 설명하지 못한다. 동작 구현과 지원 표시를 함께 갱신해야 한다.
- `scripts/probe_codex_session_delivery.py`: app-server 프로브는 제품 연결 코드가 아니다.
  과거 모의 모델 재개 결과를 현재 일반 TUI attach 성공으로 확대하지 않는다.

## 1단계 — 준비 상태와 지원 범위 표시 정정

대상: `manage.rs`, `mcp.rs`, `connection.rs`, `runners.rs`, 관련 문서.

1. 로컬/원격 MCP 등록, 선택 바인딩, 송수신 접속 상태, 자동 전달 어댑터,
   실제 호스트 연결 상태, 실기 검증 여부를 구분해 표시한다.
2. 기존 저장된 Desktop 연결의 disconnected를 현재 CLI MCP의 실패로 표시하지 않는다.
   반대로 MCP 응답 성공을 자동 수신 준비 완료로 표시하지 않는다.
3. 어댑터·호스트 능력을 입증할 수 없는 연결 요청에는 지원 불가와 정확한 준비 절차를 반환한다.
   공유 MCP 환경의 세션 ID·프로세스 이름만으로 호스트를 추정하지 않는다.
4. Claude Channels 모드에서는 불필요한 Desktop connect로 유도하지 않게 도구 안내를 구분한다.
5. 로컬 MCP 등록에 OAuth 로그인을 요구하지 않는다. 기존 원격 MCP를 자동으로 덮어쓰지 않는다.

완료 기준: 일반 Codex CLI에서 상태 확인만으로 자동 수신 준비 완료가 나오지 않고,
Claude Channels의 adapter-ready와 실제 receipt 관측이 별도로 표시된다.

## 2단계 — Codex CLI 입력 경로 선정

로컬에서 확인한 버전은 `codex-cli 0.153.4`, Claude는 `2.1.263`이다.
읽기 전용 `--help`에서 다음 기능을 확인했다. 메시지 주입 명령은 실행하지 않았다.

- `codex queue --thread <UUID> --message <TEXT>`: 기존 세션에 메시지를 큐잉하는 명령.
  도움말만으로 유휴 TUI 자동 실행·중복 방지·전달 결과 추적을 보장할 수 없다.
- `codex --remote <endpoint>` 및 `codex app-server --listen <endpoint>`:
  TUI와 어댑터가 같은 app-server 실행체를 공유할 수 있는 후보.
- app-server의 `turn/start`, `turn/steer`, 상태·이벤트 조회:
  지정 작업 입력과 실행 관측을 위한 후보 API.

**우선 `queue`의 기존 TUI 전달 의미를 격리 환경에서 검증한다.** 단순히 저장만 하는지,
같은 살아 있는 TUI가 자동 실행하는지, 다른 실행체를 만드는지 분리한다.
정확한 UUID·실행 소유자 확인, 입력의 외부 데이터 구분, 결과 식별·대조가 가능해야 채택한다.
로컬 도움말에 queue가 있지만 조사한 공식 CLI 페이지에는 해당 항목을 찾지 못했다.
최소 버전·실제 의미는 설치 버전의 스키마/구현과 격리 시험으로 확정한다.

queue가 계약을 충족하지 못하면 **공유 app-server + 공식 TUI의 remote 연결**을 선택한다.
사용자가 그 경로로 시작한 TUI와 수신 어댑터가 동일 실행체·작업을 공유하도록 런처를 설계한다.
Windows는 로컬 WS, Unix는 사용자 전용 소켓을 우선 검토하고 접근 통제·호환성을 검증한다.
공유 실행체의 승인 요청이 TUI에 표시되고 어댑터가 대신 승인하지 않는지도 확인한다.

두 후보 모두 기존 일반 TUI에 무조건 attach할 수 있다고 약속하지 않는다. 재실행이 필요하면
명시적으로 안내하고 기존 실행체 종료를 확인한 뒤 정확한 작업을 이어간다.
같은 작업을 다른 app-server에서 동시에 resume하거나 최신 작업을 추정하지 않는다.

완료 기준: 격리 시험에서 같은 TUI의 유휴 턴 시작·문맥 보존·busy 중 대기·승인 유지·
명확한 실패와 불명확 결과를 검증하고 지원 OS/버전과 시작 조건을 문서화한다.
이 단계가 통과하기 전에는 제품 어댑터 API나 출시 버전을 확정하지 않는다.

## 3단계 — Codex CLI 제품 어댑터

대상 후보: 새 `codex_cli.rs`, `connection.rs`, `delivery.rs`, `mcp.rs`, `manage.rs`, `main.rs`.
파일 구조·CLI 명칭은 2단계에서 선택한 전송 방식에 맞춰 확정한다.

1. 연결 레코드에 정확한 실행 endpoint·작업 UUID·실행 세대·adapter를 저장한다.
   기존 Desktop 레코드·저널 호환성을 유지하고 임의 대상 전환은 거부한다.
2. 바인딩별 단일 수신 소유자를 유지한다. 별도 worker와 MCP가 각자 JOIN해 서로
   테이크오버하지 않도록, 동일 Client 또는 로컬 중계로 MCP 발신과 비동기 수신을 통합한다.
   이 소유권 설계는 worker 구현보다 먼저 확정한다.
3. 수신 ID 중복 제거, 영속 저장, 제출 전 submitting, 관측 가능한 수락 기록을 구현한다.
   CLI 종료 코드 0이나 queue 등록만으로 모델 처리 완료를 기록하지 않는다.
4. 첫 구현은 독립 메시지를 실행 중인 턴에 무조건 steer하지 않고 순서대로 대기시킨다.
   유휴 판정 경합과 사용자 취소·승인 대기를 처리한다. 상관 답신 steer는 별도 검증 후 추가한다.
5. timeout·응답 유실은 unknown으로 보존하고 자동 재제출하지 않는다.
   메시지 ID와 작업 이력 대조 및 근거를 요구하는 단건 수동 복구를 제공한다.
6. 외부 payload는 신뢰하지 않는 데이터로 전달한다. 모델·작업 폴더·권한을 변경하지 않는다.
   종료한 세션을 자동으로 다른 작업이나 headless 실행으로 대체하지 않는다.

## 4단계 — Claude Channels 사용 흐름 보완

기존 수신 어댑터를 다시 작성하지 않는다. 아래 준비·진단을 먼저 보완한다.

- 일반 MCP와 Channels 설정을 구분하는 설정 생성/검사 경로를 제공한다.
  실행 파일·설정 경로·바인딩을 고정하고 중복 MCP·worker·daemon 소유 가능성을 표시한다.
- Channels를 켜고 시작해야 한다는 점을 연결 안내에 반영한다. 이미 실행 중인 일반 세션에
  런타임으로 채널을 붙일 수 있다고 안내하지 않는다. 개발 채널 확인·조직 정책은 사용자에게 남긴다.
- `channel_status`에서 adapter-ready, 호스트에서의 수신 관측, timeout/이전 세션 복구 필요를 구분한다.
  실제 호스트의 Channels 활성화를 확인할 API가 없으면 미확인으로 표시한다.
- busy/승인 대기 중 60초 receipt timeout을 실기에서 확인한다. timeout을 늘려 문제를 숨기지 않고
  늦은 receipt, 일시 중단, 수동 복구의 동작과 사용자 안내를 함께 검증한다.

## 선행 프로토콜 검토

PROTOCOL.md 13.3은 후속 행동 착수 확인 후 ACK와 실행체 종료 시 실패 보고를 규정한다.
현재 Channels 코드는 모델 receipt 전에 영속 저널 인계 후 ACK한다. 새 CLI 어댑터에서
이를 그대로 확대하기 전에 영속 인계가 규정의 착수에 해당하는지, 종료·실패 보고 책임이
어디에 있는지를 명확히 해야 한다. 현재 구현을 근거로 스펙을 준수한다고 단정하지 않는다.

영속 인계를 정식 계약으로 채택한다면 로컬 보존·복구·오류 가시화 및 실패 보고 조건을
PROTOCOL.md/PROTOCOL.en.md에 먼저 명시한 후 구현한다. 기존 규정을 유지한다면
그 규정에 맞춰 ACK 경로를 수정한다. 주소·서버의 에이전트 큐·단일 활성 세션은 유지하며,
공유 wire 스키마 변경은 현재 계획의 전제가 아니다. 필요해지면 schemas도 함께 갱신한다.

## 검증과 출시 순서

1. 1단계 준비 상태 정정과 프로토콜 결정.
2. Claude 설정/진단 보완 및 사용자가 수행하는 실제 Channels 단방향 수신 시험.
3. 격리된 Codex queue/공유 app-server 적합성 시험으로 경로 선정.
4. Codex 어댑터 및 단일 수신 소유권 통합, 회귀 시험.
5. 사용자가 수행하는 CLI ↔ CLI 양방향 실기, 이후 지원 OS별 확대.

자동 회귀: 호스트 오인 방지, 미지원 endpoint, 잘못된 작업/세대, 중복 JOIN 방지,
동시 수신, 순서, 중복 메시지, 기록 실패, submitting 중 종료, 응답 유실, late receipt,
승인 대기·취소, 기존 Desktop 및 일반 MCP 호환성을 누적 관리한다.
각 구현 후 `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`를 수행한다.

실기 성공 기준: 최초 지시 이후 수신자에게 추가 입력·폴링 없이 같은 세션에서 자동 턴 시작,
요청에 없는 기존 기억 표식 보존, 원래 correlation 답신, 발신 세션의 실제 수신을 확인한다.
방향별 단방향 시험을 먼저 통과한 후 자동 왕복을 3회 반복한다.
busy·승인 대기·종료/재시작은 정상 왕복과 별도 시험으로 기록한다.
앱 버전·OS·실행 옵션·작업 ID·메시지/턴 ID·시각·실제 결과를 남긴다.
개발 에이전트는 사용자 실기 세션의 송수신·수동 깨우기·설정 변경에 개입하지 않는다.

README.md/README.en.md, CURRENT_TASK_CONNECTION.md, PLATFORM_CONNECTION.md,
CLAUDE_CHANNEL.md 및 릴리스 문서에 구현/실기 검증/미지원 범위를 분리한다.
최소 버전과 지원 범위가 확정되고 회귀·실기가 통과한 뒤 출시한다.

## 공식 참고 자료

- [Claude Channels 규격](https://code.claude.com/docs/en/channels-reference): capability·stdio 알림과 개발 채널 활성화 조건.
- [Codex App Server](https://learn.chatgpt.com/docs/app-server): 지정 thread의 turn 시작·steer 및 승인 요청.
- [Codex CLI](https://learn.chatgpt.com/docs/cli/reference): remote TUI와 app-server endpoint 연결.

공식 문서와 로컬 도움말은 인터페이스 존재의 근거이며 Brevduva 실기 성공의 증거는 아니다.

## 이번 계획 작업의 검증

`cargo test --workspace --locked --quiet`: 전체 95개 통과, 실패 0개.
제품 소스·사용자 설정은 변경하지 않았고 실기 송수신도 수행하지 않았다.


## 개발 후속 기록

- 프로토콜 원문·영문판에 영속 인계 ACK 계약을 명시했다. wire 스키마는 변경하지 않았다.
- `codex_cli.rs`는 지정 loopback app-server에서 로드된 정확한 작업만 확인한다.
  작업 생성/resume·별도 headless 실행 없이 `turn/start.toolOutput`으로 전달한다.
- `codex queue`는 격리 실측에서 모델 턴을 시작했지만 사용자 입력 경로다.
  제품은 외부 데이터를 도구 결과로 유지하는 app-server 경로를 선택했다.
- `brv mcp --codex-cli-endpoint`와 현재 작업의 `receiver_connect(thread_id)`를 추가했다.
  초기화 때 작업을 추정하지 않고, 호스트 확인 후 하나의 Client로 수신·발신한다.
- `receiver_session_status`, 모드별 도구 목록, `setup --runner codex|claude`를 추가했다.
  setup은 설정 예시만 출력한다. Claude의 수신 어댑터는 기존 구현을 유지했다.
- receipt 상태를 재사용하고 Codex 제출 turn ID를 보존한다. 다른 작업으로 복구 이전은 거부한다.
  모델 제출 pause 중에도 영속 수신하고, pump 실패 시 Client 접속을 종료한다.
- Windows Codex 0.153.4의 실제 TUI와 새 brv를 로컬 모의 모델/WS에 연결해 동일 작업의
  자동 턴, 도구 결과 입력, 문맥, TUI 출력, 단일 접속의 영속 ACK·receipt·reply/hops를 확인했다.
  재현 도구: `crates/brv/tests/tools/probe_codex_cli.py`.
- 실제 모델의 receipt·업무 수행·Claude 왕복, 승인/취소 실기, macOS/Linux TUI는 남아 있다.
  따라서 정식 지원 또는 출시 완료로 표시하지 않는다. 사용자 실기는 사용자가 진행한다.
- 사용자 설정·시험 세션·서비스·설치 바이너리를 변경하지 않았다. 커밋·태그·배포도 하지 않았다.

개발 소스 사용법: [Codex CLI](CODEX_CLI.md), [Claude Channels](CLAUDE_CHANNEL.md).

최종 검사: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test --workspace --locked --quiet` 통과. 회귀 105개, 실패 0개.
최신 개발 바이너리로 Windows 실제 TUI + 모의 모델 통합 프로브도 재통과했다.
Codex/Claude setup 출력 명령은 격리 설정으로 실행 확인했다.
