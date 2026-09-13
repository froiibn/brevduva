# 리시버 재구축 작업 계획 (RECEIVER_REBUILD_PLAN)

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

[RECEIVER_DESIGN.md](RECEIVER_DESIGN.md)의 원칙 P1–P9를 코드로 옮기는 작업 계획이다. 설계 의도의
진실은 RECEIVER_DESIGN.md이고, 이 문서는 **어떤 순서로 무엇을 만드는가**만 소유한다. 착수:
2026-09-09.

## 0. 무엇이 바뀌는가 (한 장)

```
[지금 0.6.39]                          [목표]
서버                                    서버
 ▲  ▲  ▲  ▲   ← 넷이 각자 JOIN해 자리 다툼    ▲        ← 리시버 하나만 JOIN
 │  │  │  │                              │
 │  │  │  └ connection worker (GUI)      리시버 서비스
 │  │  └─── brv status (CLI)             ├ 바인딩별 Client(WS)
 │  └────── brv mcp (세션, 리시버 로직 통째)  ├ Router (P5·P6)
 └───────── brv daemon (서비스)           ├ Registry (P7 become·hold)
                                         └ 로컬 엔드포인트 (P3, loopback+토큰)
                                              ▲      ▲        ▲
                                         Claude   Codex   (로봇 제어기)
```

핵심 전환은 **"세션이 서버에 붙는다" → "세션이 리시버에 붙는다"** 하나다. 나머지는 그 결과다.

## 1. 확장성·로봇 로드맵 반영 (착수 전 못박기)

PLAN 2026-08-25 "개인 개발자 → 기업 B2B → 로봇 에이전트" 로드맵을 로컬 평면 설계에 미리 반영한다.
나중에 뜯어고치지 않기 위해 **처음부터** 아래를 지킨다.

1. **로컬 평면은 MCP를 모른다.** 등록부·라우터는 `Envelope`와 세션 능력만 안다. MCP/HTTP는
   프런트엔드 하나일 뿐이고, 로봇 제어기는 같은 등록부에 다른 프런트엔드로 붙는다.
2. **세션 능력 선언은 PROTOCOL 4장 `Capabilities`와 같은 모양.** `content_types`·`encodings`·
   `modes`·`meta`를 그대로 받는다 — 로봇 단계의 `cbor`·바이너리 페이로드가 필드 신설 없이 들어온다.
3. **프런트엔드는 트레이트 뒤.** `SessionSink`(리시버→세션 밀어넣기)는 전송 수단을 감춘다.
   HTTP/SSE·유닉스 소켓·명명 파이프·시리얼(로봇)이 같은 자리에 꽂힌다.
4. **한 머신 = 리시버 1개, 세션 N개, 바인딩 M개**를 불변식으로. 로봇 여러 대를 한 게이트웨이가
   중계하는 배치가 이 불변식 안에 그대로 들어간다.
5. **정책은 로컬 신뢰 정책**(2026-08-30 결정 유지) — 원격 메시지가 로컬 평면 정책을 못 바꾼다.
   깨운 세션 여부를 env 변수가 아니라 **등록부가 안다**(현행 `BREVDUVA_BINDING` 판정보다 강하다).

## 2. 단계

각 단계는 그 자체로 녹색(빌드·clippy·테스트 통과)이어야 하고, 회귀 테스트를 남긴다.

| # | 단계 | 산출 | 원칙 |
|---|---|---|---|
| 1 | **세션 등록부** — attach/become/hold/evict/detach 순수 로직 | `local_plane/registry.rs` | P4·P7 |
| 2 | **로컬 접속 자격** — 엔드포인트 파일·토큰·OS별 권한 | `local_plane/auth.rs` | P3·U5 |
| 3 | **로컬 엔드포인트** — loopback HTTP, MCP Streamable HTTP 프런트엔드 | `local_plane/http.rs` | P3 |
| 4 | **도구 표면 이관** — 도구 처리를 세션 프로세스에서 리시버로 | `local_plane/tools.rs` | P2·P3 |
| 5 | **라우터** — 붙은 세션 push / 무인 깨우기 / 큐 보관 | `local_plane/router.rs`, `daemon.rs` | P5·P6 |
| 6 | **세션 어댑터 전환** — `brv mcp`가 stdio↔로컬 브리지로, 서버 JOIN 제거 | `mcp.rs`, `main.rs` | P2·P3 |
| 7 | **밀어넣기 경로 이관** — Claude Monitor·Codex queue를 리시버 소유로 | `session_delivery.rs` | P5 |
| 8 | **CLI 이관** — `status` 등이 서버가 아니라 리시버에 질의 | `main.rs` | P2 |
| 9 | **잔재 제거** — 세션 쪽 `idle_park`·`takeover_standby` 사용·단일 바인딩 고정 삭제 (2026-09-11 정정: 데몬 쪽은 유지) | `client.rs` | P4·P7 |
| 10 | **버전 악수·갱신** — 옛 어댑터 차단, 잔재 정리 | `service.rs`, `install.*` | P8 |
| 11 | **러너 능력 3칸 표기** — 깨우기/CLI 유인/GUI 유인 × 실측 여부 | `runners.rs` | §3 |
| 12 | **실기 검증** — Claude·Codex 실제 왕복, 3 OS | 테스트·문서 | P9 |
| 13 | **전달 연장·연기 프로토콜** — `WORKING`·`DEFER`, JOIN `delivery` 조건, 연기는 격리 집계 제외 (2026-09-11) | PROTOCOL 7.2·12.2·13.4, `brevduva-protocol` frame·schemas, 서버 ws·poison·config | P4·P6 |
| 14 | **확정 시점 정정** — ACK는 에이전트 수신 증거에만: 넘기는 동안 `WORKING`, 큐 보관·결과 불명은 `DEFER`, queue id 확정 폐지, 깨우기 ACK = `become(wake)` | `client.rs`, `plane.rs`, `daemon.rs` | P4·P5·P6 |
| 15 | **수동 수신** — 평면의 `wait_for_message`·`wait_for_reply`, 임대 90초·답 우선·대기 상한 45초·취소 알림 | `plane.rs`, `bridge.rs` | P4 |
| 16 | **기동 조건** — `[wake]` 없이 기동, 받을 곳이 있을 때만 서버 자리, `--attended-only` 뜻 변경 | `daemon.rs`, `main.rs` | P1·P5 |
| 17 | **`brv listen` = 리시버 관찰** — 로컬 엔드포인트의 읽기 전용 사건 흐름 | `http.rs`, `plane.rs`, `main.rs` | P2·P5 |
| 18 | **원격 MCP 대기 상한 점검** — 기본 60초·최대 120초가 호스트 제한(Claude Code HTTP 첫 응답 60초 등)과 겹치는지 확인·정정 | 서버 `mcp_remote.rs` | P4 |

의존: 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8·9 → 10 → 11 → 12. 8·9는 6·7 뒤 병행 가능.

## 3. 단계별 완료 조건

- **1**: 등록부 단위 테스트 — become 밀어내기, hold 중 become 거부, hold 해제 조건(최종
  reply/report), 세션 사망 시 hold 해제, 다중 바인딩, 강제 해제.
- **2**: 토큰 생성·검증·권한이 3 OS에서 의도대로. 유닉스 0600, 윈도우 ACL. 상수 시간 비교.
- **3**: 로컬 MCP 엔드포인트에 붙어 `initialize`/`tools/list`가 돌고, 잘못된 토큰·Origin은 거부.
- **4**: 기존 도구(send/reply/request/…)가 리시버 쪽에서 동작. 세션은 중계만.
- **5**: 붙은 세션 있으면 push, 없으면 깨우기, 러너 없으면 큐. 수락 없으면 ACK 안 함.
- **6**: 다중 바인딩 머신에서 대화형 세션이 붙는다(0.6.39 검토 1번 해소). 등록 args에 `--binding` 없음.
- **7**: 세션 프로세스가 죽어도 밀어넣기 경로가 리시버에 남는다.
- **8**: `brv status`가 채널 슬롯을 건드리지 않는다.
- **9**: 세션 쪽 사용 소멸(hard delete). 2026-09-11 정정: 데몬의 standby(PROTOCOL 2.2)와 `wait_wake` 중 파킹은 유지. 14단계 뒤 깨우기가 수신 루프를 막지 않아 파킹은 안전장치로만 남는다.
- **10**: 갱신 뒤 옛 어댑터가 붙으면 거부되고 이유가 보인다.
- **11**: `brv status`와 문서가 같은 3칸 표를 쓴다.
- **12**: 실제 모델 왕복 기록.
- **13**: 서버 통합 시험 — `WORKING` 중에는 재전송·격리가 없고 최대 기간 뒤에는 재전송된다, `DEFER`는 지연 뒤 재전달되고
  격리 집계에서 빠진다, 잘못된 대상은 아무것도 바꾸지 않고 거부된다, JOIN에 `delivery`가 실린다.
- **14**: 모델 receipt 전에는 확인하지 않으면서도 긴 대기에서 격리되지 않는다. queue id·스폰 성공만으로 확인하지 않는다.
- **15**: 통로 없는 세션이 기다리는 동안 받고, 임대 안 빈틈의 메시지가 깨우기로 새지 않으며, 요청한 세션의 답이
  우선한다. 대기 상한을 넘기지 않고, 취소된 대기에는 넘기지 않는다.
- **16**: 깨우기 설정이 없는 머신에서 리시버가 뜨고, 세션이 바인딩을 쥘 때만 서버에 붙는다.
- **17**: `listen`이 수신을 방해하지 않고 행선지를 보여 준다.
- **18**: 원격 MCP 대기가 호스트 제한 안에서 끝난다.

## 4. 진행 기록

| 날짜 | 단계 | 상태 | 산출 |
|---|---|---|---|
| 2026-09-09 | 계획 수립 | 완료 | 이 문서 |
| 2026-09-09 | 1. 세션 등록부 | 완료 | `local_plane/registry.rs` — become 밀어내기·hold·깨우기 창 승계·다중 바인딩. 시험 25건 |
| 2026-09-09 | 2. 로컬 접속 자격 | 완료 | `local_plane/auth.rs` — OS 난수 토큰·상수 시간 대조·소유자 전용 기술서·Origin 검증. 시험 8건 |
| 2026-09-09 | 3. 로컬 엔드포인트 | 완료 | `local_plane/http.rs` — 루프백 MCP Streamable HTTP, SSE가 곧 "받을 수 있음". 시험 12건 |
| 2026-09-09 | 4. 도구 표면 이관 | 완료 | `local_plane/plane.rs` — become/receipt/list_bindings + 기존 도구. 폴링 도구 없음 |
| 2026-09-09 | 5. 라우터 + 데몬 연결 | 완료 | `plane.rs` `route()` + `daemon.rs` 배치 라우팅·깨우기 창 잠금·`BREVDUVA_WAKE` 주입. 시험 23건 |
| 2026-09-09 | 6. 세션 어댑터 전환 | 완료 | `local_plane/bridge.rs` — `brv mcp`가 브리지. 서버 JOIN 없음, `--binding` 폐지, 버전 악수. 종단 시험 5건 |
| 2026-09-09 | 8. CLI 이관 (일부) | 진행 | `brv status`가 리시버의 `/status`를 읽는다 — 더는 JOIN하지 않는다. `brv send`·`request` 등 발행 명령은 아직 직접 JOIN |
| 2026-09-10 | 7. 착수 전 실측 | 완료 | 러너 입력 전달은 사용자 명의로 — §5. 3단계의 P4 초과 판정 발견 |
| 2026-09-10 | 7a. Monitor 이관·수신자 판정 정정 | 완료 | `plane.rs` 개편 — 러너 입력 통로(`DeliveryTarget`)·세션당 순차 전달·전달 기록·결과 불명 자동 재주입 금지·`receiver_connect`/`receiver_resolve` 평면 도구. `registry.rs` 통로 세대 대조, `http.rs` SSE는 제어 통로로. 시험 180건 녹색(로컬 평면 반복 실행 포함) |
| 2026-09-10 | 7b. Codex queue 이관 | 완료 | `runner_exec.rs` 신설(사용자 명의 실행기 — 윈도우 서비스 winspawn·그 외 직접), `plane.rs` Codex 작업 통로·제출 결과 반영·적재 감시, `session_delivery.rs` 헬퍼 공유, `bridge.rs` 프로필 문맥 주입. 시험 195건 녹색 |
| 2026-09-10 | 관리 명령 명의 회귀 수정 | 완료 | `plane.rs` `run_management` — 관리 도구와 codex-desktop 연결이 `manage::run_cli`(서비스 명의 자식) 대신 사용자 명의 실행기로. 시험 추가 |
| 2026-09-10 | 7c. Claude Channels 이관 | 완료 | `registry.rs` `TargetKind::Channels`·`PushEvent::ChannelEvent`, `http.rs` `notifications/claude/channel` 표기, `plane.rs` Claude 호스트 선언·확인 사건·채널 전달 작업. 사용자 문서(CLAUDE_CHANNEL.md)는 옛 경로 삭제(7e)와 함께 갱신. 시험 203건 녹색 |
| 2026-09-11 | 7d. Codex Desktop 이관 | 완료 | `registry.rs` `TargetKind::CodexDesktop`, `plane.rs` 소유자 확인·Desktop 전달 작업·`handed`(바쁨 대기 중 세션 종료는 되돌림)·turn id 기록, `desktop.rs` 리시버 전용 `submit` 도우미(턴 요청 전 실패=넣지 않음, 보낸 뒤=결과 불명), `main.rs` 숨은 `desktop submit`, `bridge.rs` Desktop에도 프로필 문맥. Desktop 미실행으로 실측 불가 — 실제 앱 검증은 12단계. 사용자 문서(DESKTOP_RECEIVER.md)는 7e에서 갱신. 시험 213건 녹색 |
| 2026-09-11 | 7e. 레거시 삭제 | 완료 | `connection.rs`·`claude_channel.rs`·`codex_cli.rs` 삭제, `mcp.rs`는 인자 도우미만, `session_delivery.rs`의 세션 소유 pump·`QueueTarget`, `desktop.rs`의 worker·자체 기록, `main.rs`의 `connect`·`connection`·`desktop run/resolve/status`·`mcp setup`·실험 플래그, `manage.rs`의 `receiver_connection`·`run_cli`·환경변수 무인 판정, 설치기의 `connection restart`. `daemon.rs` 기동 때 옛 연결 의도 회수(`retire_legacy_adapters`). 기록 시험은 `delivery.rs`로 옮김. 사용자 문서: 옛 사용법 5개 삭제, `RECEIVING.md` 신설, README 한·영 갱신. 시험 184건 녹색, 서버 리포 `cargo check --tests` 통과 |
| 2026-09-11 | 9. 잔재 제거(범위 축소) | 완료 | 세션 쪽 `idle_park`·`takeover_standby` 사용은 7e로 소멸. 클라이언트의 두 옵션은 데몬이 쓰므로 유지 — PROTOCOL 2.2 standby 규정, `wait_wake` 중 파킹(RECEIVER_DESIGN P4 정정) |
| 2026-09-11 | 깨어난 세션 정체성 회귀 수정 | 완료 | `daemon.rs` `build_prompt`가 깨우기 식별자와 `become` 지시를 싣는다(Codex는 MCP 자식에 환경변수를 넘기지 않음), 폴링 지시 삭제, 옛 상태 파일 폴백 `waking_binding` 삭제 |
| 2026-09-11 | 13~18 계획 | 결정 기록 | 받는 주체=에이전트(확인 유보의 격리 위험 발견), 수동 수신 정식화, 기동 조건, `listen` 관찰, 원격 MCP 대기 점검 — RECEIVER_DESIGN P4·P5·P6, CLAUDE.md 원칙 4·5·6·10, PROTOCOL 7.2·12.2·13.4 개정 |
| 2026-09-11 | 13. 전달 연장·연기 프로토콜 | 완료 | `brevduva-protocol` `WORKING`·`DEFER` 프레임·JOIN OK `delivery` 조건(`DeliveryTerms`)·스키마 스냅샷(추가만). 서버: 설정 `working_max`·`defer_min`·`defer_max`, 격리 원장 `forgive`(연기는 집계 제외), ws 연장·연기 처리(원자적, 잘못된 대상 거부), HTTP long-poll은 거부. 서버 시험 `delivery_extension` 5건, 서버 전체 스위트 녹색 |
| 2026-09-11 | 14. 확정 시점 정정 | 완료 | `client.rs` 연장·연기 요청과 서버 조건 추적. `plane.rs` `Routed::Defer`(넘길 곳 없음·결과 불명), 넘긴 뒤 receipt까지 `WORKING`, queue id 확정 폐지(runner 표식만), 결과 불명 해소는 그 바인딩을 쥘 수 있는 세션이면. 깨우기: 평면의 수신 증거 신호(`become(wake)`·`brv send`의 `BREVDUVA_WAKE`), `bridge.rs` `claim_wake`, `daemon.rs` 깨우기를 별도 작업으로(수신 루프 비차단, 진행 중 새 몫은 15초 연기, 증거까지 `WORKING`, 증거 없으면 1회 재깨우기 뒤 미확인·실패 보고, 연장 수단 없는 서버는 스폰 직후 확정). 서버 깨우기 시험 5개는 가짜 러너가 식별자를 남기고 시험이 대신 증명하도록 갱신. brv 시험 187건·서버 전체 스위트 녹색 |
| 2026-09-11 | 15. 수동 수신 | 완료 | `plane.rs` 수동 수신 몫(세션·바인딩별 대기열·임대 90초·기다리는 호출 알림), 라우터가 입력 통로 없는 홀더를 기다리는 중·임대 안·자기 요청의 반응일 때 받는 곳으로 셈(몫은 `WORKING`, 임대 종료·바인딩 상실·세션 종료 시 `DEFER`), `wait_for_message`·`wait_for_reply` 도구(상한 45초, 돌려줄 때 기록·확정, 진행 알림은 progress, 첨부 report는 원문으로 판정, 입력 통로가 붙은 세션은 `push_mode` 거부), `notifications/cancelled` 처리(넘기지 않고 응답하지 않음), `request`가 통로 없는 세션의 요청을 기억. `bridge.rs` 세션이 선 뒤 요청을 동시에 전달(대기 중 취소·다른 도구 통과). 사용자 문서 `RECEIVING.md` 수동 수신 절. 서버 시험 `manual_receive` 신설. brv 시험 191건·서버 전체 스위트 녹색 |
| 2026-09-11 | 16. 기동 조건 | 완료 | `daemon.rs` `[wake]` 없이 기동, `wake_dir` 없는 바인딩도 건너뛰지 않음, 바인딩 루프가 받을 곳(사전 점검 통과한 깨울 러너·바인딩을 쥔 로컬 세션)이 있을 때만 접속 — 점검 실패 중에도 세션이 쥐면 접속, 받을 곳이 사라지면 유예(`leave_grace`, 기본 30초) 뒤 내려놓음, 깨울 수 없을 때 온 무인 몫은 30초 `DEFER`, 운영 중 깨우기 시작 실패는 쥔 세션이 없을 때만 관문 복귀. `client.rs` 상태 `Dormant`. `plane.rs` `binding_held`, 막 쥔 직후 도구 호출의 접속 대기(최대 10초). `main.rs` `init --attended-only`(와 대화형 "아니오") = 깨우기 없이 리시버 서비스 등록. 사용자 문서 README 한·영·`RECEIVING.md`. 서버 시험 `receiver_without_wake` 신설. brv 시험 192건·서버 전체 스위트 녹색 |
| 2026-09-11 | 17. `brv listen` = 리시버 관찰 | 완료 | `plane.rs` 관찰 흐름(broadcast, 구독자 없으면 싣지 않음) — 받은 메시지 요약·행선지(`route` 판단을 감쌈)·수락(receipt·manual_receive)·결과 불명. `daemon.rs` 깨우기 결과 사건(시작·증명·증명 없음·시작 실패·연기). `http.rs` 운영자 표면 `GET /listen`(줄 단위 JSON, 유휴 빈 줄, 밀림은 건수). `main.rs` `brv listen`이 `/listen`만 읽음(사람용 줄·`--json`·`--binding`), 리시버가 없으면 말하고 종료, 옛 JOIN 경로·전용 도우미 삭제. 서버 시험 `manual_receive`에 관찰을 켠 채 수신·행선지·수락 사건 확인 추가. brv 시험 194건·서버 전체 스위트 녹색 |
| 2026-09-11 | 18. 원격 MCP 대기 점검 | 완료 | 실측(로컬 느린 MCP 서버 + `claude -p` 2.1.263): JSON 한 번 응답 50초 통과·75초는 **60.0초에 연결 끊김**, SSE(헤더 먼저·keep-alive) 75·150초 통과. 번들 코드: 응답·진행 없는 HTTP MCP 호출 300초에 중단. 정정: PROTOCOL 5.3·9장·12.2(한·영) "원격 MCP 대기 도구 한 번 최대 45초", 서버 설정 `mcp_wait_max`(기본 45초)로 박힌 상한 120초·기본 60초 대체, 도구 설명에 상한 표시. SSE는 Codex 도구 호출 제한(기본 60초)을 풀지 못해 미채택. 서버 단위 시험 `waits_stay_under_the_host_tool_call_limit`, phase18·서버 전체 스위트 녹색 |
| 2026-09-12 | 10. 버전 악수·갱신 잔재 | 완료 | `service.rs` `sweep_parked_binaries`(실행 파일 옆 `brv.old`·`brv.exe.old`·`brv.exe.old.<id>` 청소 — 쓰는 중이면 다음 기회), 리시버 기동·`brv daemon restart`에서 호출. `http.rs` 브리지 버전 헤더(`x-brv-bridge-version`) 대조 — 다르면 409와 "이 앱의 MCP를 재시작" 사유, 헤더 없는 직접 HTTP 클라이언트는 수용. `bridge.rs` 모든 요청에 버전. `daemon.rs` 옛 기록 판정을 `legacy_leftovers`로 분리(기동 로그·`brv status` 공용), STANDBY 설명에 원인·조치. `main.rs` `brv status`에 이전 리시버의 끝나지 않은 기록. 재구축 이전 `brv mcp`(서버 직접 JOIN)는 리시버가 막을 수 없어 STANDBY 안내로 드러낸다. 사용자 문서 `RECEIVING.md`. brv 시험 197건·서버 전체 스위트 녹색 |
| 2026-09-12 | 11. 러너 능력 3칸 표기 | 완료 | `runners.rs` 옛 `AttendedDelivery`(turn-end hook / passive) 삭제, 러너마다 `attended_cli`·`attended_gui`(`Push::None` 또는 통로 이름+실측 여부)와 `wake_capability()`. Claude — 깨우기 실측·CLI Monitor·Channels(미실측)·GUI 없음, Codex — 깨우기 실측·CLI codex queue(미실측)·GUI Codex Desktop(미실측), 그 밖 19종 — 밀어넣기 없음. `main.rs` `brv status` 러너 줄에 세 칸. 서버 리포 RUNNERS.md 같은 표(옛 분류 번복 기록). brv 시험 197건(세 칸 독립·정직성)·서버 전체 스위트 녹색 |
| 2026-09-12 | 12. 실기 검증 (1차 — 이 윈도우 머신) | 진행 | 운영 서비스·배포 없이 로컬 시험 서버(격리 인프라)·임시 프로필 포그라운드 리시버 둘·모델 없는 발신 MCP 클라이언트. 실제 러너: Claude Code 2.1.263 무인 깨우기 왕복(`become(wake)` 1.1초 증명·착수 알림·reply) 성공, 수동 수신(입력 통로 없는 `claude -p`) 성공, Monitor 밀어넣기(전달→2초 receipt→reply) 성공, Channels는 `claude -p` 턴 종료로 미실측, Codex(0.153.4 npm `codex.cmd`) 깨우기는 **`.cmd` 감싸기 결함으로 실패**(프롬프트 첫 줄 뒤 유실 — RECEIVER_DESIGN §4 U7), 14단계 재깨우기 상한·failed 보고 실측, `brv listen`·`brv status` 실제 출력·`dormant`→세션 접속 확인. 반영: `runners.rs` 통로별 실측(`Push::Paths` — Claude Monitor measured), 낡은 주석 3곳. RUNNERS.md 표. brv 시험 197건·서버 전체 스위트 녹색. 남은 실기: Channels·codex queue(대화형), Codex Desktop, U7 수정 뒤 Codex, 서비스 모드(운영 교체 승인), macOS·Linux |
| 2026-09-13 | U7 수정 + 12. 실기 검증 (2차 — Codex) | 진행 | 사용자 승인(권고안): 프로필 인자에 `{prompt}`가 없으면 프롬프트를 **표준 입력**으로(`runners.rs` Codex `exec … -`, `daemon.rs` 프롬프트 파일→stdin 두 스폰 경로, `winspawn.rs` stdin 핸들, `main.rs` `wake show`·`wake test`가 `.cmd`+인자 전달 설정 경고). 착수 전 실측: Codex 직접·`cmd /c codex.cmd` 감싸기·Claude `-p` 모두 표준 입력 4줄 완전 도달. 구현 중 실측: 프롬프트 파일을 열어 둔 채 스폰 전에 지우면 Node 경유 자식이 빈 입력을 봄 → 다음 깨우기 때 청소로 변경. 실기: 격리 `CODEX_HOME` 래퍼로 Codex 0.153.4 무인 깨우기 왕복 성공(요청→깨우기→18초 뒤 `become(wake)` 증명→reply). brv 시험 198건·서버 전체 스위트 녹색. 남은 실기: Channels·codex queue(대화형), Codex Desktop, 서비스 모드(운영 교체 승인), macOS·Linux |
| — | 8·10·11·12 | 진행·미착수 | 아래 참조 |

### 다음에 할 일 (남은 단계의 현재 상태)

- **8. CLI 이관** — `brv status`·`brv send`는 리시버로 옮겼다. `brv listen`도 17단계에서 리시버 관찰 명령으로
  바꿨다 — CLI 이관 완료.
- **13~18** — 2026-09-11 결정(받는 주체=에이전트·수동 수신·기동 조건·listen). 13(프로토콜)·14(확정 시점)·15(수동 수신)·16(기동
  조건)·17(listen)·18(원격 MCP 대기)은 끝났다. 남은 것은 10·11·12단계, 배포는 사용자 확인 후.
- **10. 갱신** — 끝났다(2026-09-12): `.old` 잔재 청소, 요청마다 브리지 버전 대조·거부, standby 원인 안내,
  끝나지 않은 옛 기록의 `brv status` 표시.
- **11. 러너 능력 3칸 표기** — 끝났다(2026-09-12): `brv status`와 서버 리포 RUNNERS.md가 같은 세 칸(무인 깨우기 /
  CLI 유인 밀어넣기 / GUI 유인 밀어넣기, 실측 여부 포함)을 쓴다. 밀어넣기 칸의 "실측"은 12단계에서 채운다.
- **12. 실기 검증** — 1차(2026-09-12, 이 윈도우 머신·격리 인프라): Claude 무인 깨우기·수동 수신·Monitor 밀어넣기
  실제 왕복 성공, Channels는 대화형 세션 필요로 미실측, Codex 깨우기는 윈도우 `.cmd` 감싸기 결함으로 실패 → U7 수정(표준 입력)
  뒤 2차(2026-09-13)에서 실제 Codex 왕복 성공. 남은 것: Channels·codex queue(대화형 세션), Codex Desktop(앱 실행),
  윈도우 서비스 모드(운영 서비스 교체 승인 필요), macOS·Linux 머신.
- **발견된 열린 문제(2026-09-11)**: ① (해소, 15단계) Claude Code Stop 훅이 평면에 없던 `wait_for_message`를
  안내했다 — 수동 수신이 정식 모드로 돌아와 안내가 다시 유효하다(입력 통로가 붙은 세션은 `push_mode`로 거부되고 사건으로 받는다) ② (해소, 14단계) 데몬이 깨운 세션의 완주를 기다리는 동안 수신 루프가 멈춰, 그 사이 온
  메시지는 창이 끝난 뒤에야 라우팅됐다 — 확인 없는 대기라 격리될 수도 있었다. 깨우기를 별도 작업으로 뗐다 ③ 브리지가 SSE 조각마다 UTF-8로 해독해
  여러 바이트 글자가 조각 경계에 걸리면 깨질 수 있다(지금 싣는 사건은 ASCII). ④ (해소, 2026-09-13) 윈도우 `.cmd` 러너 깨우기가 여러 줄
  프롬프트를 잃던 결함 — 프롬프트를 표준 입력으로 넘긴다(RECEIVER_DESIGN §4 U7) ⑤ (2026-09-12) 이 머신의 사용자 Codex
  설정에 옛 등록(`~/.local/bin/brv.exe` 0.6.39 `mcp --binding personal/brvcodex@brv`)이 남아 있다 — 새 브리지는
  `--binding`을 거부하므로 갱신 뒤 `brv mcp register`로 다시 등록해야 한다(사용자 조치)

### 이 변경이 요구하는 사용자 조치 (마이그레이션)

`brv mcp`가 브리지가 되면서 **등록에 `--binding`이 있으면 거부**된다(있으면 이유를 말하고 종료).
- 데몬이 깨우는 세션은 자동이다 — 깨우기용 MCP 설정 파일을 리시버가 매번 새로 쓴다.
- 손으로 등록된 것(예: 이 개발 머신의 `~/.codex/config.toml`)은 `brv mcp register`를 다시 실행해야
  한다.
- 리시버 서비스가 떠 있어야 세션이 붙는다. 없으면 `brv mcp`가 그 사실을 말하고 종료한다.
- 옛 `brv connect`로 연결한 Codex Desktop 작업은 새 리시버가 기동할 때 연결이 거둬진다(옛 worker 정지) — 그 작업
  안에서 `receiver_connect(session_kind="codex-desktop")`로 다시 붙인다.
- `brv mcp --claude-channel`·`--codex-cli-endpoint`로 만든 세션 소유 등록은 더는 뜨지 않는다(알 수 없는 인자) —
  `brv mcp register`로 다시 등록하고 세션 안에서 `receiver_connect`를 쓴다.

## 5. 7단계 세부 계획과 착수 전 실측 (2026-09-10)

### 5.1 실측 결과

사용자 지시("구현 전에 실측으로 확인")에 따라 코드를 옮기기 전에 측정했다. 상세 수치와 판단은
[RECEIVER_DESIGN.md](RECEIVER_DESIGN.md) P5 "러너 입력 전달의 실행 명의"에 있다. 요약:

| 측정 | 결과 |
|---|---|
| `codex queue`의 동작 | CODEX_HOME 위에 앱 서버를 프로세스 안에서 띄워 `thread/queue/add` — 사용자 프로필에 파일을 만든다 |
| CODEX_HOME ACL | SYSTEM·Administrators·사용자 모든 권한(상속) |
| `codex-ipc` 파이프 DACL | SYSTEM·Administrators·사용자 모든 권한, Everyone·ANONYMOUS 읽기 |
| LocalSystem 직접 실행 | 관리자 권한이 없어 측정하지 못함 |
| 소스(`rust-v0.153.4`) | Windows는 항상 프로세스 안 앱 서버, 공유 매체는 `queue_1.sqlite`, 적재 프로세스가 약 10초 폴링, 호출자 신원 검사 없음 |
| 소스 — Unix 제어 소켓 | 0700 디렉터리 안 0600, 피어 자격 검사 없음 → 같은 사용자여야 한다 |
| 소스 — `codex-ipc` | Desktop(Electron) 비공개 코드라 확인 불가 → 7d 착수 전 실측. 2026-09-11: Desktop 미실행으로 실측 불가, 서비스가 IPC를 열지 않는 설계로 쟁점 해소(RECEIVER_DESIGN P5) |

결론: 러너 입력 통로에 넣는 실행체는 **로그온 사용자 명의로** 실행한다(Windows 서비스는 winspawn,
Linux·macOS는 직접). SYSTEM 실행의 성공 여부와 무관하게 사용자 프로필 안의 행동은 사용자 명의라는
2026-09-03 결정을 따른다. 루프백 Monitor 스트림처럼 파일을 만들지 않는 통로는 서비스가 직접 연다.

### 5.2 측정 중 발견한 결함 — 3단계의 P4 초과 판정

SSE 스트림이 열리면 곧 수신자로 표시했다. 일반 MCP 호스트는 알림으로 모델 턴을 열지 않으므로, 그런
세션이 붙어 있으면 메시지가 수락되지 않은 채 재전달만 반복되고 무인 깨우기로도 가지 않는다(기아).
7a에서 수신자 판정을 **러너 입력 통로 준비**로 바꾸고 SSE는 제어 통로로만 쓴다.

### 5.3 세부 단계

| # | 내용 | 완료 조건 |
|---|---|---|
| 7a (완료 2026-09-10) | 수신자 판정 정정 + 세션별 전달 대상(Monitor 우선) + 리시버 소유 Monitor 리스너 + `receiver_connect`를 평면 도구로 + 세션당 한 번에 하나씩 전달(수락 전 다음 없음) + 제출 기록(불확실 결과 자동 재주입 금지, 운영자 확정) | 통로만 연 세션은 수신자가 아니다(무인 경로). Monitor가 붙은 세션은 수신자이고, 수락 전 다음 전달이 없고, 세션이 사라진 제출은 확정 전까지 재주입되지 않는다 |
| 7b (완료 2026-09-10) | Codex queue — 사용자 명의 실행기(Windows winspawn / 그 외 직접) 경유 | 서비스 프로세스가 `codex queue`를 SYSTEM으로 실행하는 코드 경로가 없다 |
| 7c (완료 2026-09-10) | Claude Channels — 평면의 initialize가 선언을 붙이고 알림 어휘를 맞춘다. 준비는 확인 사건의 수락으로 판정 | 선언만으로 수신자라고 표시하지 않는다(수락 관측 전까지 미확인) |
| 7d (완료 2026-09-11) | Codex Desktop worker 흡수 — 서비스는 IPC를 열지 않고 사용자 명의 도우미로만(신원 검사 확인 전제는 이것으로 해소) | 평면 경로에 worker가 없다. 옛 worker 코드 삭제는 7e |
| 7e (완료 2026-09-11) | 레거시 삭제(`--claude-channel`·`--codex-cli-endpoint`·세션 소유 pump) + 9단계(`idle_park`·`takeover_standby`) | 해당 코드 hard delete, 회귀 녹색 |
