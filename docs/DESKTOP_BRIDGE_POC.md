# 기존 Codex Desktop 작업으로 메시지 전달 PoC

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

## 목적과 범위

Brevduva는 벤더 독립적인 에이전트 통신망이다. 이번 시험은 리시버 바깥의
실행 어댑터가 기존 작업에 입력을 전달할 수 있는지 검증한다. 공개 주소,
에이전트 소유 큐, 단일 활성 접속, 프로토콜 및 서버 구현을 변경하지 않는다.
서버의 세션별 큐 신설을 전제하지 않으며, 이전 SESSION_DELIVERY.md의
확장안은 이번 PoC의 채택된 설계가 아니다.

## 확인한 사실

- `brv listen`을 백그라운드로 유지하면 턴 종료 후에도 메시지가 출력된다.
- 그러나 출력만으로는 기존 Codex 모델 턴이 시작되지 않았다. 3차 시험은
  사용자가 확인을 지시한 후에야 모델이 읽었다.
- 현재 Windows Desktop의 내부 IPC `thread-owner-discovery`가 선택한 작업의
  소유 실행체를 반환하고 `supportsUntrustedAppInput=true`를 응답했다.
- 설치 코드의 `thread-follower-start-turn` 처리 경로를 확인했다. 외부 입력은
  현재 턴이 진행 중일 때 명시적으로 거부되며, 이 거부만 재시도 대상으로 삼는다.

아래 실제 시험에서 입력 수락·기존 작업의 모델 실행·reply 발행까지 확인했다.

## 시험 구성

`scripts/poc_brv_desktop_bridge.cjs THREAD BINDING SENDER NEW_STATE_DIR`

1. 사람이 지정한 정확한 작업 ID와 바인딩·발신자를 사용한다.
2. 기존 CLI `brv listen`으로 받은 envelope를 로컬 시험 기록에 저장한다.
3. 지정한 발신자의 직접 request 하나를 선택하고 수신 프로세스를 종료한다.
4. Desktop 소유 실행체를 찾아 외부 메시지 컨텍스트로 입력을 전달한다.
5. 현재 턴 진행 중이라는 명시적 거부에만 3초 간격으로 최대 10분 재시도한다.
6. 입력 수락, 불명확한 결과 및 거부를 기록한다. 자동 재실행·재개는 없다.

수신 대기는 최대 15분이다. 한 번에 한 건만 전달하는 실험이며 상시 서비스가
아니다. 기존 작업의 모델·작업 폴더·권한 설정을 상속한다. 원격 payload를
사용자의 지시로 승격하지 않고 외부 데이터로 표시한다.

## 성공 기준

사용자가 다음 메시지를 입력하지 않은 상태에서 Claude의 새로운 request가
도착하고, 이 기존 Codex 작업이 다음 턴을 시작해 이전 시험 표식을 포함한
reply를 보낸다. `accepted`만으로 성공을 선언하지 않는다. 실제 대화 기록의
새 턴과 Brevduva reply를 함께 확인한다.

## 한계

- Windows의 설치된 Desktop 내부 IPC만 대상으로 하며 안정된 공개 API가 아니다.
- 기존 CLI가 수신을 소비한 뒤 파일 저장 전에 죽는 손실 구간이 남아 있다.
  로컬 기록은 운영용 영속 인계 구현을 대신하지 않는다.
- 비대상 메시지도 CLI에서 소비될 수 있어 모든 envelope를 기록한다.
  전용 시험 바인딩에서만 사용한다.
- 타임아웃 또는 연결 단절은 수락 여부 불명확으로 남겨 사람이 이력을 대조한다.
- ACK, 보고, broadcast, 여러 작업 라우팅, 승인 대기·사용자 취소와 장기 복구는
  이번 시험의 검증 범위 밖이다. 새 헤드리스 작업으로 우회하지 않는다.

## 검증 기록

- 어댑터 테스트 4개 통과: 외부 데이터 구분, 지정 소유자 전달, 미지원 소유자
  차단, 명시적 busy 거부만 재시도.
- 공개 저장소 `cargo test --workspace`: 72개 통과.
- 실제 시험: 2026-09-06 UTC, Windows Desktop에서 한 건 성공.

### 실제 실행 결과

대상 기존 작업: `01a071db-ba10-7ec0-8c03-0d218b3ee206`.
Claude의 시험 표식은 `BRV_DESKTOP_WAKE_D7E2`다.

| 단계 | 근거 |
|---|---|
| Claude request | `01M1VDZGAVAM99KME8W0AKV1V2` |
| 로컬 수신 | `2026-09-06T13:19:30.608Z` |
| Desktop 수락 | `2026-09-06T13:19:30.845Z` |
| 기존 작업의 새 턴 | `01a076df-c1c9-7592-9a38-aa51e5b76425` |
| 실제 모델 실행 | 자동 시작된 턴에서 payload를 읽고 로그 확인 및 답장 도구 실행 |
| reply 발행 | `01M1VE04JM6XBTE2GTVEZ60HEQ`, 원래 request ID를 correlation으로 사용 |

이전 턴의 최종 응답 뒤 사람이 메시지 확인을 요청하지 않아도 실행됐다.
어댑터가 외부 앱 문맥과 고정된 일반 프롬프트를 제출했고, 대화 기록에는
그 프롬프트가 user 역할로 보인다. 사람이 직접 입력한 메시지와 구별해야 한다.

상태 기록은 `target/desktop-bridge-20260906-221827/` 아래에 있다.
로컬 수신부터 Desktop 수락까지는 같은 머신 시계로 약 237ms다.
서버 시각과 로컬 시각 사이에는 차이가 있어 이를 네트워크 지연으로 해석하지 않는다.

앞선 세 표식을 답장에 포함했다. 다만 Claude의 요청에도 그 세 표식이 적혀 있어,
표식 재현만으로 독립적인 기억 보존을 증명하는 시험은 아니다. 동일 작업 ID와
현재 대화 문맥에서 실행된 사실은 확인했다.

이 결과는 한 번의 자동 턴 시작과 reply 발행 성공이다. Claude 측 실제 수신은
별도 확인 대상이며, 상시 반복 동작·장애 복구·다른 OS/CLI 지원은 미검증이다.
PoC 수신기는 한 건을 받은 뒤 종료됐다.

## 후속: 3회 연속 전달 시험

`poc_brv_desktop_rounds.cjs THREAD BINDING SENDER NEW_STATE_DIR`가 단건 어댑터를
최대 3번 순서대로 실행한다. 각 수락 이후 다음 수신기를 열고 이전 메시지 ID는
제외한다. 거부·불명확·프로세스 실패 시 반복을 중단한다. 각 수신 대기는 최대
15분이며 전체 시험은 상시 서비스가 아니다. 라운드별 기록을 따로 보관한다.

Claude는 직접 request를 보내고 reply를 받은 뒤 다음 request를 보내야 한다.
요청마다 서로 다른 표식을 사용한다. 세 번의 입력 수락만으로 왕복 성공을
선언하지 않고 각각의 실제 reply 발행 및 상대 수신 기록을 대조한다.
작업 중 도착하면 기존 어댑터의 명시적 busy 거부 처리를 사용한다.
### 3회 시험 결과 (2026-09-07 KST / 2026-09-06 UTC)

추가 사용자 입력 없이 같은 Desktop 작업에 세 번의 새 턴이 시작됐고,
각 턴에서 원래 request를 correlation으로 지정한 reply를 발행했다.

| 회차 | 표식 | request ID | reply ID |
|---|---|---|---|
| 1 | BRV_SEQ_1_9A3E | 01M1VSWVJC8E30FRYGB59QZYZX | 01M1VSX2V0XT2CRG5FD59R0H59 |
| 2 | BRV_SEQ_2_7C1B | 01M1VSX9WCGDPSMAPXEG3WTY7F | 01M1VSXJYNZBQAF3DA8R81P7MR |
| 3 | BRV_SEQ_3_E5F0 | 01M1VSXTV1JD1CR11FWYE4NY5F | 01M1VSY42YJM1WF0BRXE38BNRG |

Desktop turn ID는 순서대로 `01a0779e-6eef-7031-b77d-35217d12ae75`,
`01a0779e-a7ef-7693-b3a8-6c77906d6e6e`, `01a0779e-ebea-7573-86ac-a8270e18eb26`이다.
2·3차 요청 본문에는 이전 표식의 값이 없었고, Codex는 기존 대화에서 찾아 답했다.

기록: `target/desktop-rounds-20260907-014655/` 및 같은 이름의 stdout 로그.
감독 프로세스 PID 13588은 유지됐고 단건 브리지 PID는 27724 → 1672 → 24724로
교체됐다. 세 건 수락 후 감독 프로세스가 종료된 사실을 확인했다. 계속 수신하는
상시 서비스나 단일 수신 프로세스의 연속성은 이번 결과로 주장하지 않는다.

관측 기록에서 세 요청은 각각 한 번 수락됐고 세 reply 발행을 확인했다.
Claude 측 최종 수신 확인과 Desktop의 실제 화면 렌더링은 별도 확인 대상이다.
명시적 busy 거부 로그는 이번 시험에 없으므로 작업 중 도착 처리의 실측은 남아 있다.
추가 회귀 테스트 포함 Node 테스트 7개, 공개 저장소 Rust 테스트 72개가 통과했다.

### 두 번째 세트 재시험

새 감독 프로세스와 `target/desktop-rounds-20260907-015003/` 기록 폴더로
같은 조건을 반복했다. 세 건 모두 추가 사용자 입력 없이 기존 작업의 새 턴을
시작하고 reply를 발행했다.

| 회차 | 표식 | request ID | reply ID |
|---|---|---|---|
| 1 | BRV_SEQ2_1_4B8D | 01M1VT2ZRKQZRARXA432X210VQ | 01M1VT37WW58PB6DSF18NGNNQF |
| 2 | BRV_SEQ2_2_C0E7 | 01M1VT3EPHK2F2EE6XTGJ3R2HD | 01M1VT3PANF6076N8W79596TWB |
| 3 | BRV_SEQ2_3_A91F | 01M1VT3Y5ZTS5B171GQY0ZQH44 | 01M1VT4A40GMVJ8HHKYQQY1SP8 |

두 세트 로그를 대조한 결과 수락은 각각 3건, 어댑터의 busy 재시도·거부·불명확·
fatal·listener-error·만료·invalid-line 이벤트는 0건, stderr는 비어 있었다.
이는 하위 네트워크 계층의 재시도가 전혀 없었다는 보장은 아니다.
2세트 감독 PID 26568도 세 건 수락 후 종료됐다. 작업 중 도착 경합은 여전히
실측하지 않았으며, 최종 reply의 상대 수신은 Claude 측 기록과 대조해야 한다.

### 세 번째 세트 (사용자 녹화용 재시험)

기록 폴더는 `target/desktop-rounds-recording-20260907-015343/`이다.
추가 사용자 입력 없이 세 요청이 기존 작업의 새 턴으로 전달됐고 reply를 발행했다.

| 회차 | 표식 | request ID | reply ID |
|---|---|---|---|
| 1 | BRV_SEQ3_1_6D2A | 01M1VT9A9BEF5QYMK66KPE89R4 | 01M1VT9J017V71C3JE494CDDSK |
| 2 | BRV_SEQ3_2_F17C | 01M1VT9RJRQPAHJF2CK2E5R84M | 01M1VTA0A5R6WSRWAMCZQSQBNZ |
| 3 | BRV_SEQ3_3_0B5E | 01M1VTA7RFM85BDC6X1FSBYVP1 | 01M1VTANKGBTND3CCVA7BBQQWH |

어댑터 로그: 수락 3건, busy 재시도 0건, 거부·불명확 0건, 실패 이벤트 0건,
stderr 비어 있음. 감독 PID 1456은 세 건 수락 후 종료됐다.
세 세트 합계 9회의 자동 턴 시작 및 reply 발행을 관측했다.
녹화 파일 자체는 Codex가 생성하거나 확인하지 않았다. 상대의 최종 수신 확인,
작업 중 도착, 장애 복구 및 다른 플랫폼에 관한 미검증 범위는 그대로다.
