# 기존 Codex Desktop 작업 연결 — 리시버 실험 통합

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

## 프로젝트 경계

Brevduva의 벤더 독립적인 통신 프로토콜, 에이전트 주소와 큐 귀속, 단일 활성
접속 규칙을 유지한다. 이 기능은 `brv`의 로컬 실행 어댑터다. 서버 변경이나
새 세션 큐, 러너에 관한 공개 프로토콜 필드를 도입하지 않는다.

PoC의 Node.js 실행 체인을 Rust 클라이언트와 OS별 로컬 IPC 호출로
대체했다. 기존 헤드리스 데몬은 그대로 유지된다. 현재 통합 범위는 같은 사용자
계정으로 실행하는 `brv desktop run`이며 LocalSystem 서비스의 자동 실행·설정
UI·설치 패키지 배포는 포함하지 않는다. 세션을 소유한 Desktop이 켜져 있어야 한다.

## 사용법

현재 소스를 `cargo build -p brv`로 빌드한다. 배포된 기존 바이너리와 구별한다.

```powershell
.\target\debug\brv.exe desktop check --thread <기존-작업-ID>
.\target\debug\brv.exe desktop run --binding brvcodex@brv --thread <기존-작업-ID>
.\target\debug\brv.exe desktop status --binding brvcodex@brv
```

- `--thread`는 사람이 선택한 정확한 작업 ID다. 최근 작업 추정·새 작업 생성·
  다른 실행체에서 같은 ID 재개는 하지 않는다.
- 수신은 Ctrl+C까지 계속된다. 시험 시 `--max-deliveries 1`로 입력 수락 수를 제한할
  수 있다. 수락 한도는 응답 완료 횟수가 아니다. 한도 종료 전에 보관한 나머지는
  다음 실행에서 같은 작업 ID로 재개한다.
- 프로필은 `brv desktop --config C:\경로\config.toml run ...`으로 지정한다.
- `check`와 `status`는 채널에 JOIN하거나 모델 입력을 보내지 않는다.
- 다른 수신 세션이 접속 자리를 가져가면 기존 클라이언트의 standby 규칙을 따른다.
  같은 바인딩에서 기존 MCP 수신을 함께 사용하면 접속 인계에 따른 지연이 생길 수 있다.

## 메시지 인계

1. Desktop 소유 실행체와 외부 입력 지원을 확인한 뒤 채널에 접속한다.
2. `recv_manual`로 받고, 바인딩별 저널에 메시지 전체와 대상 작업을 기록해
   `sync_all`을 완료한다. 성공한 다음에만 `confirm`으로 transport ACK를 보낸다.
3. 입력 제출 전에 `submitting`을 기록한다. Desktop의 `thread-owner-discovery`로
   소유자를 찾아 `thread-follower-start-turn`을 호출한다.
4. 턴 ID가 있는 성공 응답을 받으면 `accepted`와 턴 ID를 저장한다.
5. 현재 턴이 실행 중이라는 명시적 거부만 `pending`으로 되돌려 3초 후 재시도한다.
   기다리는 동안에도 수신·저장은 별도 future에서 계속된다.
6. 그 밖의 오류나 응답 유실은 `unknown`으로 보관하고 종료한다. 재시작 시
   `submitting`이나 `unknown`이 있으면 자동 재실행하지 않는다.

접속 자체가 실패하면 제출 전 메시지는 `pending`으로 남는다. Desktop 소유자는
10초마다 확인하며 사라지면 수신도 종료해 채널 자리를 반납한다. 토큰 오류 등
클라이언트 종료도 오류로 드러낸다. 시스템 이벤트는 기록하되 모델을 깨우지 않는다.

## 보관과 신뢰 경계

설정 디렉터리의 `desktop/<org 또는 legacy>/<agent>/<channel>/deliveries.jsonl`에
추가 기록한다. 토큰은 기록하지 않는다. 서버·완전한 바인딩 정체성을 헤더에서
검사한다. 별도 잠금 파일로 같은 바인딩의 Desktop 수신기 중복 실행을 막는다.
운영 상태 조회는 메시지 본문을 출력하지 않는다.

메시지 ID 중복은 재시작 후에도 제거한다. 보관된 pending을 다른 작업 ID에
넘기지 않는다. 손상된 마지막 미완성 줄만 복구하며, 완성된 줄의 손상은 오류다.
Windows에서는 상태 조회를 막지 않도록 잠금과 데이터 파일을 분리한다.

원격 메시지는 외부 앱의 신뢰하지 않는 문맥으로 전달한다. 기존 작업의 모델,
작업 폴더, 승인·샌드박스 설정을 상속하며 메시지로 변경하지 않는다. 무인 러너가
이 모드를 임의로 시작하는 것은 기존 로컬 정책 변경 가드로 차단한다.

## 검증과 남은 범위

기존 PoC는 9회 순차 자동 턴 시작 및 reply 발행을 확인했다. 그 결과를 Rust
통합의 실제 왕복 성공으로 대신하지 않는다.

추가 테스트 9개: 재시작 중복 제거, pending 작업 고정, submitting 재실행 차단,
미완성 accepted 기록 복구, 단일 수신 소유자, 실행 중 상태 읽기, 정체성·손상 검사,
외부 문맥과 권한 상속, 실제 비동기 스트림의 IPC 프레임 및 단절 분류를 포함한다.
기존 72개와 합쳐 81개가 통과했다. 빌드한 바이너리로 실제 Desktop owner 조회 성공.

제한:

- Windows named pipe 및 macOS·Linux Unix 소켓을 구현했다. 내부 IPC는 안정된 공개 API가
  아니다. Windows 실제 왕복과 Linux 모의 소켓/worker 테스트를 구별하며 macOS 실기기
  실행은 미검증이다. 임의의 CLI 기존 세션 연결을 지원한다고 표시하지 않는다.
- `accepted` 이후 모델의 완료·답장·UI 렌더링을 감시하지 않는다.
- 알 수 없는 결과는 자동 해결하지 않는다. 아래 수동 복구 절차로 저널과 Desktop 이력을
  대조하며, 기록 삭제나 무조건 재시작을 복구 방법으로 삼지 않는다.
- 사용자 취소를 지속 정지로 해석하는 정책과 승인 대기 중 후속 요청 처리는 미확정이다.
  현재 active 턴은 대기하나 별도의 취소 상태를 감지하지 않는다.
- 전원 장애 내구성, 강제 종료 시점별 통합 시험, 저널 용량 제한·정리는 추가 작업이다.
  로컬 파일 보관을 exactly-once 실행 보장으로 표현하지 않는다.
- 로컬 바인딩이 지정 작업 전체를 맡는다. 여러 작업의 correlation 소유권 분배는
  구현하지 않았으며 그 구성을 지원한다고 주장하지 않는다.

## 실제 통합 시험: 작업 중 수신

2026-09-07 KST, 빌드한 Rust `brv desktop run`을 이 기존 Desktop 작업에 연결했다.
Node.js PoC 프로세스는 사용하지 않았다.

- Claude request: `01M1VTRA2DDH2XZSSKXMD9MED3`, 표식 `BRV_NATIVE_BUSY_20260907`.
- 디스크 보관: `2026-09-06T17:04:16.778909Z`.
- 기존 모델 턴이 진행 중이라 `17:04:16.805096Z`부터 `17:04:38.013773Z`까지
  명시적 busy 거부 8회. 요청은 pending으로 유지됐다.
- 기존 턴 종료 후 `17:04:41.211728Z` 입력 수락. 새 턴 ID는
  `01a077ad-e8ac-72e0-af5c-d85a105ec188`.
- 추가 사용자 입력 없이 같은 작업에서 실행돼 Claude에게 reply
  `01M1VTW7PHNAJXEZCTZZD2PMT2`를 발행했다.
- 실행 중 `desktop status` 조회도 성공했다. 입력 수락 후 제한 1건에 따라
  프로세스는 종료됐다. Claude 측 reply 수신 확인은 별도다.

같이 수신된 로컬 loopback 메시지 `01M1VTT540946HFBHYEFY6P8PY`는 한도 종료 후에도
저널에 pending으로 남았다. 이를 재시작 복구 시험 대상으로 사용한다.

## 실제 재시작 복구 시험

같은 작업 ID와 저널로 리시버를 다시 실행했다. 이미 수락한 Claude 요청은
다시 제출하지 않았고, 남은 loopback 메시지만 선택했다.

- 재시작: `2026-09-06T17:05:15.028272Z`.
- 현재 모델 턴에 대해 busy 거부 4회 후 `17:05:27.402913Z` 수락.
- 새 턴: `01a077ae-9d21-7432-872f-1f79da51cc14`.
- 추가 사용자 입력 없이 기존 작업에서 실행됐고, 저널의 두 메시지는 각각 한 번씩
  accepted로 기록됐다. pending 0건이며 시험 수신 프로세스는 종료됐다.

이는 정상 종료 후 pending 복구 시험이다. 전원 차단이나 IPC 응답 직전 강제 종료를
실제로 주입한 시험으로 확대 해석하지 않는다. 두 실행의 경로·프로세스 기록은
`target/desktop-native-busy-result.json`, `target/desktop-native-current.json`에 있다.

## 불명확한 전달 복구

먼저 `brv connection pause --binding org/agent@channel`로 worker를 정지한다.
`brv desktop status --binding org/agent@channel`의 메시지 ID와 정확한 작업 ID를 확인하고,
그 작업의 이력에서 실제 입력 수락 여부를 대조한다. 확인할 수 없으면 정지 상태를 유지한다.

이미 입력이 수락되었다면:

```sh
brv desktop resolve --binding org/agent@channel --id <메시지-ID> --accepted-turn <확인한-턴-ID> --note "이력 대조 근거" --confirm
```

입력이 수락되지 않았음을 확인한 경우에만:

```sh
brv desktop resolve --binding org/agent@channel --id <메시지-ID> --retry --note "미수락 확인 근거" --confirm
```

이 명령은 submitting/unknown만 변경하고 결정 근거를 저널에 추가한다. 자동으로 메시지를
보내지 않는다. 모든 불명확한 건을 해결한 뒤 `brv connection resume --binding org/agent@channel`로
재개한다. retry는 원래 작업을 유지하며, 잘못된 판단이면 중복 작업이 발생할 수 있다.
accepted는 작업 완료를 뜻하지 않는다.
