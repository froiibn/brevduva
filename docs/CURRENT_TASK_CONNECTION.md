# 현재 작업에 연결 — 사용자 흐름과 구현

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

## 목표

사용자는 GUI·CLI의 차이나 작업 ID를 알 필요 없이 현재 작업 에이전트에게
"Brevduva에 연결해줘"라고 요청한다. 로컬 호스트 식별과 러너별 전달 어댑터가
그 차이를 처리한다. Windows named pipe와 macOS·Linux Unix 소켓을 구현했다.
실제 Codex Desktop 왕복은 Windows, 모의 소켓·worker 실행은 Linux WSL에서 검증했다.
macOS 실행은 아직 미검증이다.
공통 사용 흐름을 만들었다는 사실을 모든 GUI·CLI 지원으로 표현하지 않는다.

## 에이전트의 실행 흐름

아래 `brv connect` 흐름은 **Codex Desktop**에 적용한다. v0.6.34의 Codex CLI는
공유 app-server를 명시한 MCP에서 `receiver_connect(thread_id=...)`를 사용한다.
일반 CLI에서는 Desktop worker를 시작하지 않는다. [CLI 준비 절차](CODEX_CLI.md).
Claude는 [Channels 시작 설정](CLAUDE_CHANNEL.md)을 사용하며 connect를 실행하지 않는다.

1. 사용자가 수신 주소를 지정했으면 그대로 사용한다. 바인딩이 하나면 자동 선택,
   여러 개이고 지정하지 않았다면 주소를 고른다. 작업 ID를 사용자에게 요구하지 않는다.
2. 현재 작업의 셸에서 `brv connect --binding agent@channel`을 실행한다.
3. 이 명령은 호스트의 `CODEX_THREAD_ID`를 읽고 실제 Desktop 소유 실행체를 확인한다.
   세션 ID가 없거나 지원 실행체가 아니면 실패 이유를 반환한다. 최근 대화·포커스·
   기록 검색으로 작업을 추정하거나 새 실행체를 대신 만들지 않는다.
4. 연결 대상을 디스크에 기록하고 현재 사용자 계정으로 백그라운드 수신기를 시작한다.
5. 상태가 `receiving`일 때 해당 채널에 접속해 실제 수신 중이다. `connecting`,
   `standby`, `waiting_for_app`, `needs_attention`을 구별한다.

`receiver_connect` MCP 도구는 실행 파일·인자·설정 경로를 구조화해 반환한다.
에이전트는 이를 현재 작업의 셸에서 실행한다. **MCP 프로세스는 대화 간 공유될 수
있으므로 그 환경의 세션 ID를 현재 호출자의 ID로 간주하지 않는다.** 연결 요청에
이미 사용자 권한이 있으므로 단순 셸 전달 때문에 재확인을 요구할 필요는 없다.
호스트가 셸에도 세션 식별자를 제공하지 않으면 지원 불가로 알린다.

## 제어

| 요청 | 실행 |
|---|---|
| 이 작업에 연결 | `brv connect --binding agent@channel` |
| 연결 상태 | `brv connection status --binding agent@channel` |
| 수신 일시정지 | `brv connection pause --binding agent@channel` |
| 저장된 연결 재개 | `brv connection resume --binding agent@channel` |
| 연결 해제 | `brv connection disconnect --binding agent@channel` |

MCP 제어 도구는 `receiver_connection(action=...)`이며 CLI와 같은 경로를 사용한다.
등록된 MCP 바이너리를 업데이트하고 호스트의 도구 목록을 새로 읽어야 새 도구가 보인다.
기존 배포 바이너리나 실행 중인 MCP 프로세스가 자동으로 바뀌지는 않는다.

이미 같은 작업에 연결돼 있으면 새 worker를 중복 실행하지 않는다. 다른 작업이면
기존 대상을 알리고 사용자의 변경 의사를 확인한 뒤 `--replace` 또는 MCP의
`confirm=true`를 사용한다. 기존 작업의 미전달 메시지가 있으면 다른 작업에
넘기지 않는다. 연결 해제는 메시지 기록을 삭제하지 않는다.

일시정지·해제는 worker가 설정 변경을 읽고 수신을 멈추는 방식이다. PID를 찾아
다른 프로세스를 강제 종료하지 않는다. 이미 시작한 모델 턴은 중단하지 않는다.
전송 도중 중단돼 결과가 불명확하면 저널의 submitting을 남겨 자동 재전송을 막는다.

## 구현 경계

- `connection.rs`: 현재 작업 식별, 연결 설정, 사용자 계정의 worker 시작 및 제어.
- `desktop.rs`: 기존 작업으로의 전달, 영속 보관·중복 방지, 실제 클라이언트 상태 관찰.
- `manage.rs`: MCP에서 같은 명령 제공. 기존 유인 세션 전용 검사 유지.
- 서버·주소·에이전트 소유 큐와 헤드리스 데몬은 변경하지 않는다.

바인딩별 저널 옆의 `connection.json`에 서버·바인딩·어댑터·작업·연결 의도를 저장한다.
`runtime.json`에는 관측 상태, `worker.log`에는 실행 로그를 남긴다. 토큰은 복사하지 않는다.
명령 잠금과 worker 잠금은 분리하며 세대 번호가 다른 이전 worker는 종료한다.
Desktop이 닫히면 수신 자리를 반납하고, 미확정 전송이 없으면 앱 재실행을 기다린다.
불명확한 전송·디스크 오류는 자동 복구로 숨기지 않고 주의가 필요한 상태로 보인다.

연결 설정은 유지되지만 OS 로그인 시 worker 자동 시작은 아직 포함하지 않는다.
재부팅·worker 강제 종료 후에는 `connect` 또는 `resume`가 필요하다. 임의의 일반 CLI·Claude
실행체 attach, 대화 제목 조회, 작업 선택 UI, 취소 정책의 정교화는 후속 범위다.

Unix worker는 시작 시 `setsid`로 터미널에서 분리된다. macOS·Linux의 소켓 경로와
사용자 소유권 검사 및 플랫폼 검증 범위는 [OS별 기록](PLATFORM_CONNECTION.md)에 정리했다.

## 검증

실제 현재 Codex 작업에서 작업 ID 인자 없이 연결했고 `receiving` 상태를 확인했다.
동일 작업 재연결은 멱등이며, pause에서 worker 종료, resume에서 새 worker 시작,
disconnect에서 종료와 설정 상태 변경을 확인했다. 검증 후 연결은 해제했다.

회귀 테스트는 호스트 식별자 누락·잘못된 값 거부, 이전 worker 세대 차단,
paused/disconnected 차단, 잠금 중 설정 원자 교체, MCP의 현재 작업 셸 전달,
MCP 명령 매핑을 포함한다. 연결 자체는 Desktop probe로 검증하며 이전
[실제 메시지 전달 시험](DESKTOP_RECEIVER.md)의 수신·자동 턴 경로를 재사용한다.

## CLI 오연결 방지 (v0.6.35)

일반 MCP의 `receiver_connect`는 `session_kind`를 필수로 받는다. 실제 Codex Desktop 문맥인 `codex-desktop`에서만 셸 명령을 준비하고, 실행 시 기존 exact-owner 검증을 유지한다. 일반 CLI·미확인 호스트는 셸 실행과 권한 승인 유도 없이 종료한다. CLI 전용 app-server 어댑터의 도구는 기존 `thread_id` 계약을 유지한다. 파일 접근 거부는 파일 없음과 분리해 원인을 보존하며, 접근 거부에 초기화를 권하지 않는다.

회귀: 일반 CLI·미확인 환경에서 명령 미생성/미접속, Desktop 명령 유지, 파일 오류 원인별 안내, 기존 자동 전달 어댑터 테스트를 수행한다. 일반 CLI에서 추가 시작 설정 없는 자동 수신은 미충족이며 이번 진단 수정의 성공과 구분한다.
