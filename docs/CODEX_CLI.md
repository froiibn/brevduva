# Codex CLI 기존 작업 자동 전달 (실험 기능)

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

## 일반 실행 세션: v0.6.36

로컬 MCP를 등록한 평소의 `codex` 대화에서 “자동 수신을 활성화해줘”라고 요청한다.
에이전트가 현재 작업 셸의 `CODEX_THREAD_ID`를 읽어
`receiver_connect(session_kind="codex-cli", thread_id=...)`를 호출한다.
MCP와 다른 설정 경로/설치본을 쓰는 세션이면 현재 셸의 `codex_home`·`codex_executable`도 전달한다.
사용자가 UUID를 복사하거나 app-server를 따로 시작할 필요가 없다.

실행 중인 정확한 작업의 OS writer lock과 설치본의 `queue` 명령을 확인한 후 연결한다.
queue에는 외부 본문 대신 고정 수신 안내·receipt 식별자만 넣는다. 본문은 `receipt`의
도구 결과로 읽으므로 외부 메시지를 사용자 지시로 승격하지 않는다. 큐 ID는 턴 ID나 처리
완료가 아니다. 실행 중인 작업이 종료되면 주입을 멈추며 다른 작업을 만들거나 재개하지 않는다.

Windows Codex 0.153.4의 실제 일반 TUI에서 유휴 턴 시작·원래 문맥 유지·화면 출력 확인.
receipt/reply는 모의 MCP 호스트가 호출한 통합 시험이며 실제 모델 추론 시험은 아니다.
전체 [설계와 시험 방법](NATIVE_SESSION_DELIVERY.md)을 참고한다.

## 공유 app-server 고급 설정 (v0.6.34부터)

아래는 기존 공유 app-server 전달 방식을 선택한 경우에만 적용된다. 일반 CLI 자동 수신의 필수 준비가 아니다.

Codex CLI 어댑터와 설정 출력 명령은 v0.6.34부터 제공한다. 설치 후 로컬 MCP를 재시작한다.
소스 빌드 바이너리는 `cargo build -p brv`로 만든다. Windows에서는 `target/debug/brv.exe`다.

## 지원 조건

공식 Codex TUI와 `brv`가 **같은 로컬 app-server**에 연결돼 있어야 한다. 일반 `codex`로
이미 실행한 독립 TUI에 자동으로 붙는 기능은 아니다. `brv connect`는 계속 Desktop용이다.
현재 확인한 조합은 Windows, codex-cli 0.153.4, 공유 loopback WebSocket이다.
다른 OS/버전은 실제 시험 전까지 지원 검증 완료로 표시하지 않는다.

이 고급 어댑터는 `turn/start.toolOutput`으로 외부 메시지를 도구 결과로 구분한다. 어댑터는 작업을 생성하거나 resume하지 않는다.
현재 작업의 정확한 UUID가 지정 endpoint에 로드돼 있는지 확인하며, 환경의 최근 작업·
MCP 부모의 세션 ID·대화 제목을 추정하지 않는다.

## 준비

사용자는 별도 시험 바인딩과 시험 설정을 준비한다. 같은 바인딩의 다른 MCP·daemon·
Desktop/PoC 수신기를 함께 실행하지 않는다. 설정 출력 자체는 접속·메시지 소비를 하지 않는다.

```powershell
# 설치된 v0.6.34 이상 바이너리·설정 파일·시험 바인딩으로 바꾼다.
& 'C:\Users\Jaeyoung\.local\bin\brv.exe' mcp --config 'C:\brevduva\config.toml' --binding 'personal/brvcodex@brv' setup --runner codex --endpoint 'ws://127.0.0.1:4500'
```

출력의 `codex_toml`을 **시험에 사용할 Codex 설정**에 반영한다. 기존 `brevduva` 항목이
있다면 해당 항목 전체를 검토해 교체한다. 원격 MCP의 `url`·OAuth 설정과 로컬
`command`·`args`를 같은 항목에 섞지 않는다. 명령은 사용자 설정을 자동 변경하지 않는다.
같은 설정을 읽는 다른 Codex 작업에도 적용될 수 있으므로 시험 프로필/설정의 범위를 확인한다.
로컬 MCP는 Codex OAuth 재인증을 사용하지 않으며 `brv`의 토큰 보관처를 사용한다.

서로 다른 두 터미널에서 같은 Codex 설정을 사용해 순서대로 시작한다.

```powershell
# 터미널 1: 위 MCP 설정을 읽는 실행체
codex app-server --listen 'ws://127.0.0.1:4500'

# 터미널 2: 같은 실행체를 사용하는 대화형 UI
codex --remote 'ws://127.0.0.1:4500'
```

기존 작업을 이어갈 때는 사용자가 정확한 작업을 선택해
`codex --remote <endpoint> resume <정확한-UUID>`로 연다. 다른 실행체에서 같은 작업이
동시에 실행되지 않는지 먼저 확인한다. 이 절차는 실행 중인 일반 TUI attach를 대체하지 않는다.
endpoint는 숫자 loopback 주소만 허용한다. 토큰 인증을 구성한 endpoint에는
`--codex-cli-token-env <환경변수명>`을 MCP 인자에 추가하고 그 변수를 MCP에 전달한다.
토큰 값은 명령·문서·로그에 넣지 않는다.

## 현재 작업 연결과 상태

새 TUI에서 에이전트에게 "현재 작업을 준비된 Brevduva 바인딩에 연결하라"고 요청한다.
CLI 모드의 `receiver_connect`는 다음 계약으로 동작한다.

1. 현재 작업의 셸에서 `CODEX_THREAD_ID`를 읽는다. 사용자가 UUID를 복사할 필요는 없다.
2. `receiver_connect(thread_id=...)`에 전달한다. 셸의 `brv connect`를 실행하지 않는다.
3. 정확한 작업이 설정한 app-server에 로드돼 있는지 확인한다.
4. 이 MCP가 수신 저널과 송수신 Client를 소유한다. 별도 Desktop worker는 만들지 않는다.

한 MCP가 다른 작업에 이미 연결돼 있으면 대상 교체를 거부한다. 재시작 후에는 다시 정확한
작업을 연결한다. `--codex-cli-thread <UUID>`로 명시적인 시작 대상을 고정하는 고급 설정도
있지만 해당 작업이 먼저 로드돼 있어야 한다.

| 도구/상태 | 의미 |
|---|---|
| `receiver_session_status` | 현재 MCP 방식·바인딩과 전달 모드. 서버 접속 검사는 별도이며 메시지를 소비하지 않음 |
| `codex-cli-awaiting-target` | endpoint 설정만 있음. 현재 작업 연결 필요 |
| `channel_status`의 `ready` | 어댑터 준비. 실제 모델 관측이나 업무 완료의 증거가 아님 |
| `host_delivery_observed=true` | 현재 세션의 유효한 receipt가 관측됨 |
| `awaiting_receipt` | 제출 후 모델 수신 확인 대기 |
| `needs_attention` | 불명확 전달, 이전 세션의 미처리 기록 또는 실행 오류 |
| `receiver_connection` | 일반 MCP에서 쓰는 별도의 저장된 Desktop 연결 상태. CLI 모드에서는 제공하지 않음 |

`receipt`를 호출하면 수신한 envelope의 hops가 기록돼 reply/report에 반영된다.
`request`는 발신 확인 후 반환하고 답장은 자동 전달되므로 수신 대기 도구는 제공하지 않는다.

## 보존·중단·복구

- 설정 폴더의 `codex-cli/org/agent/channel/deliveries.jsonl`에 디스크 동기화 후 서버 ACK.
  제출 전 submitting을 저장한다. ACK·입력 수락·receipt·업무 완료는 서로 다르다.
- 작업이 active이면 새 모델 입력을 기다리지만 메시지는 계속 영속 수신한다.
  idle 확인 직후 active로 바뀌는 경합은 app-server의 도구 결과 대기 경로에 맡긴다.
- 모델·폴더·샌드박스·승인 설정을 전달 요청에 덮어쓰지 않는다. 어댑터는 승인 요청에 응답하지 않는다.
- `channel_pause(paused=true)`는 모델 제출만 멈춘다. 영속 수신은 계속하며 이미 제출한 턴은 취소하지 않는다.
  `false`는 제출 재개다. 프로세스 재시작을 넘어서 pause를 저장하는 기능은 아니다.
- 60초 동안 receipt가 없으면 unknown으로 남긴다. 이후 메시지도 영속 보관하지만 모델 제출은 막는다.
  이전 세션의 미확정 기록도 자동으로 새 세션에 전달하지 않는다.
- endpoint 단절·제출 실패·저장 오류로 pump가 중단되면 Brevduva 접속도 종료한다.
  상태의 오류를 확인하고 MCP를 재시작한다. 실패한 PUB의 성공 여부를 추정하지 않는다.
- TUI를 닫아도 app-server가 살아 있으면 그 세션의 MCP가 남을 수 있다. 완전 수신 중지는
  시험용 MCP/app-server를 종료한다. 기존 Desktop 연결 해제로 CLI 수신을 중지하지 않는다.
- 불명확한 메시지는 작업 이력을 대조한 후 `channel_resolve(message_id, action, note, confirm=true)`로
  단건만 처리한다. `received`는 이전 수신의 수동 확정, `retry`는 재제출 허용이다.
  다른 Codex 작업의 미처리 메시지를 현재 작업으로 옮기는 복구는 거부한다.

## 검증 범위

Rust 회귀는 MCP 초기화 이후 연결, 단일 접속 송수신·영속 ACK·receipt·hops,
작업/endpoint 검증, busy 상태, 응답 유실 시 비재시도, 교차 작업 복구 거부를 포함한다.

`tests/tools/probe_codex_cli.py`는 실제 brv·Codex app-server를 격리 설정과 로컬 모의 WS/모델로
실행한다. Node 22 이상과 Python websockets가 필요하며 Windows TUI 관찰 옵션은 pywinpty도
필요하다. 의존성은 시험 디렉터리에 설치하고 `--deps`로 지정할 수 있다.

```powershell
python crates/brv/tests/tools/probe_codex_cli.py --codex '<codex.exe 절대경로>' --brv '<개발 brv.exe 절대경로>' --deps 'target/cli-probe-deps' --tui
```

Windows 0.153.4에서 같은 TUI의 자동 모델 턴·도구 결과 입력·이전 문맥 보존·화면 출력과
동일 Brevduva 접속의 영속 ACK·receipt·답신을 확인했다. receipt 호출은 모의 호스트가 수행한다.
실제 모델의 자발적 receipt·Claude↔Codex 왕복·실제 승인 화면 처리·다른 OS의 TUI는
사용자 실기 확인 대상으로 남아 있다. 개발자가 사용자 시험 세션에 입력해 통과시키지 않는다.

공식 근거: [app-server](https://learn.chatgpt.com/docs/app-server),
[remote TUI](https://learn.chatgpt.com/docs/cli/reference).

## 일반 실행 환경의 활성화 실패 수정 (v0.6.35)

일반 MCP의 `receiver_connect`는 `session_kind`로 실제 호스트를 구분한다. `codex-cli`이면 Desktop용 셸 명령을 반환하지 않고 `automatic_delivery=false`, `reason=codex_cli_not_configured`로 종료한다. 인자가 없으면 호스트 문맥을 먼저 확인하도록 한다. `--host codex`나 `CODEX_THREAD_ID`만으로 Desktop 여부를 추정하지 않는다. 공유 app-server 모드의 `receiver_connect(thread_id)` 계약은 유지한다.

이 수정은 잘못된 연결 시도 방지다. 평소 실행한 독립 TUI의 자동 주입 요구는 미충족 상태다. 별도 시작 옵션·수신 조회·다른 세션 생성으로 사용자 환경 시험을 통과 처리하지 않는다.
