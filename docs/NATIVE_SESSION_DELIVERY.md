# 일반 실행 세션의 자동 수신 설계 (v0.6.36)

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

## 목표와 구현 범위

사용자는 평소의 대화에서 자동 수신 활성화를 요청한다. 에이전트가 해당 세션에 어댑터를
연결하고 유휴 상태에서도 새 메시지가 같은 문맥에 들어와 처리되어야 한다. 특별한
시작 옵션·사용자의 메시지 복사·다른 대화 생성은 이 목표의 대체 수단이 아니다.

이번 변경은 일반 로컬 MCP의 `receiver_connect`를 실제 CLI 전달 경로에 연결한다.
OS별로 다른 사용자 절차를 요구하지 않는다. 임의의 GUI/웹 호스트가 외부 입력 수단을
제공하지 않는 경우까지 이 변경만으로 해결한 것은 아니다.

| 호스트 | 구현 | 검증 근거 |
|---|---|---|
| 일반 Codex CLI | 고유 queue + 정확한 작업 writer lock | Windows 0.153.4 실제 TUI·모의 모델, 원래 문맥에서 자동 턴 시작 |
| 일반 Claude Code CLI | 고유 Monitor + 로컬 일회용 스트림 | Windows 2.1.263 실제 TUI·모의 모델에서 자동 턴·receipt·reply |
| Codex Desktop | 기존 소유 작업 IPC + worker | 이전 Windows 왕복; Unix IPC 회귀 |
| macOS/Linux CLI | 동일 Rust queue/Monitor 구현 | CI 회귀 대상; 실제 해당 OS의 CLI 실기는 아직 미검증 |
| Monitor 없는 Claude, 기타 GUI/웹 | 필요한 호스트 입력 경로가 확보되지 않음 | 전체 목표의 남은 범위; 활성화 성공으로 표시하지 않음 |

현재 실제 사용자 계정으로 두 CLI 간 왕복 시험과 macOS/Linux 앱 실기는 개발 측 모의
시험과 별도다. 빌드 성공을 해당 앱의 동작 성공으로 바꾸어 보고하지 않는다.

## 활성화와 수명

로컬 `brv mcp`는 활성화 전부터 receipt/status/복구 도구를 노출한다. 호스트가 도구 목록을
다시 읽지 않아도 첫 자동 수신을 처리할 수 있다. 상태 조회는 JOIN을 만들지 않는다.
바인딩은 MCP에 등록된 정확한 바인딩을 사용하고 다른 바인딩 요청은 연결 전에 거부한다.

Codex는 현재 작업 셸의 `CODEX_THREAD_ID`와 필요한 경우 CODEX_HOME/실행 파일 경로를
에이전트가 읽어 넘긴다. 최근 작업·제목·MCP 부모 환경으로 대상 작업을 추정하지 않는다.
실행 중인 작업의 OS 잠금과 `queue --help`를 확인한다. 매 제출 전과 수신 루프에서
잠금을 재확인하며 종료된 대화를 resume하지 않는다. Windows npm 래퍼는 같은 설치본의
네이티브 exe로 해석하고 셸을 통해 외부 본문을 실행하지 않는다.

Claude Code는 `session_kind="claude-code"`(기존 `claude-cli`도 허용)로 현재 대화에서
제공되는 `Monitor` 도구를 에이전트가 호출한다. CLI/GUI 명칭 대신 실제 Monitor 제공
여부가 연결 조건이다. GUI에서의 실기 검증은 아직 없으며, Monitor 없는 GUI에는 적용되지 않는다. MCP가 발급한
loopback 주소와 1회용 52자 ticket으로 `brv session-stream`이 연결하고 고정 이벤트를
stdout에 출력한다. Monitor가 이 출력을 해당 대화에 넣는다. 헬퍼는 설정 파일·서버 인증을
읽지 않으므로 별도의 샌드박스 밖 설정 접근을 요구하지 않는다. 접속 전에는
`awaiting_monitor`, 접속 후에만 `transport_ready=true`다. 60초 안에 연결하지 않거나
Monitor/MCP가 끝나면 수신이 중단된다. 일반 백그라운드 Bash 명령은 Monitor의 대체가 아니다.

MCP는 수신과 reply/report에 동일 Client를 사용한다. 별도 수신 프로세스의 JOIN이 현재
접속을 빼앗지 않는다. 기존 일반 도구 연결은 지속 수신 연결로 전환한다. 한 MCP가 이미
다른 Codex 작업에 연결돼 있으면 대상 교체를 하지 않는다. Claude 연결의 소유자는 해당
Monitor를 실행한 대화이며 하나의 MCP/스트림을 여러 대화에 공유하지 않는다.

## 신뢰와 복구

외부 본문을 queue/Monitor 사용자 입력에 넣지 않는다. 고정된 안내·message_id·receipt_token만
알리고, 외부 envelope는 receipt의 MCP 도구 결과로 반환한다. 도구 권한을 넓히지 않는다.
원래 메시지 ID와 hops를 기록해 reply/report가 같은 요청을 가리키게 한다.

메시지는 잠긴 영속 저널에 기록한 뒤 서버 ACK를 보낸다(PROTOCOL 13.4). 서버 ACK,
queue ID, 모델 receipt, 업무 완료를 분리한다. queue ID는 turn ID로 저장하지 않는다.
전달 중 연결이 끊기면 제출 중 메시지를 Unknown으로 남기고 자동 재주입하지 않는다.
같은 세션에서 뒤늦게 온 유효 receipt는 수락한다. 이전 세션의 기록은 확인·수동 복구가 필요하다.
queue 실행과 작업 종료 사이의 경합은 호스트의 원자적 취소 API가 없어 완전히 제거되지
않는다. 이때도 다른 대화로 옮기거나 성공으로 덮지 않고 receipt/저널을 확인한다.

## 재현 가능한 개발 검증

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`를 수행한다.
누적 회귀에는 정확한 살아 있는 작업 잠금, 잘못된 UUID, 일회용 ticket, shell quoting,
외부 본문 분리, 미연결 상태의 무접속, 바인딩 불일치, Monitor ACK/receipt/reply,
전달 오류 후 Unknown/늦은 receipt, queue ID와 turn ID 구분을 포함한다.

Windows 실제 TUI 시험 도구는 다음 두 파일이다. 임시 프로필과 localhost 모의 모델/WS를
만들고 원래 사용자 설정·바인딩·서비스·설치 파일을 변경하지 않는다. 실제 모델 추론이나
외부 서비스 간 종단 시험은 아니다. Python 의존성 `pywinpty`, `websockets`를
`target/cli-probe-deps`에 설치한 개발 환경에서 저장소 루트 기준 실행한다.

```powershell
python crates/brv/tests/tools/probe_codex_native.py --brv target/debug/brv.exe
python crates/brv/tests/tools/probe_claude_native.py --brv target/debug/brv.exe
```

`--codex`/`--claude`로 설치된 네이티브 실행 파일, `--deps`로 Python 의존성 경로를 지정할 수
있다. 두 시험 모두 모의 모델이 실제 호스트의 MCP 도구를 호출해 활성화·receipt·reply를
수행한다. 원래 문맥 유지, 단일 JOIN, ACK와 상관 ID/hops를 검증한다. Codex fixture는
모의 MCP의 세 도구에 대해 호스트의 일반 “현재 세션 허용” 확인을 처리한다. 실제 환경의
도구 승인은 사용자가 정한 정책을 따른다. `approval_policy=never`로 미승인 MCP 도구를
차단한 환경에서 이를 우회하지 않는다.

Claude 모의 API에는 기능 플래그 서비스가 없으므로 fixture에 Monitor 제공 상태를
명시한다. 제품 코드가 사용자 기능 플래그를 변경하는 것은 아니다.

Claude Monitor의 공개 계약: [도구 참조](https://code.claude.com/docs/en/tools-reference),
[SDK 도구 스키마](https://code.claude.com/docs/en/agent-sdk/python).
Codex queue의 호환 계약은 설치본의 도움말과 위 실제 TUI 시험으로 검증한다.
