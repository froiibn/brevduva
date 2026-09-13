# brv 0.7.0

Copyright 2026 SEIZIA (Jaeyoung Ko)

리시버 재구축(2026-09-09~13). 설계 의도와 결정 이력은 [RECEIVER_DESIGN.md](RECEIVER_DESIGN.md), 사용법은 [RECEIVING.md](RECEIVING.md).

## 바뀐 것

- **세션은 리시버에 붙는다.** 서버에 붙는 것은 리시버(OS 서비스) 하나뿐이다. `brv mcp`는 리시버의 로컬 MCP 엔드포인트로 잇는 얇은 중계기가 됐고, 세션은 `become`으로 정체성을 잡는다(한 세션이 여러 바인딩, 같은 바인딩은 나중 세션이 이김, 요청을 수락한 세션은 최종 회신까지 잠김).
- **러너 입력 통로는 리시버 소유**: Claude Code Monitor·Channels, Codex CLI 작업 대기열, Codex Desktop 작업. 통로가 붙은 세션만 받고, 서버 확정은 모델의 `receipt` 때 한다.
- **받는 주체는 에이전트다.** 넘기는 동안 서버에 `WORKING`으로 알리고, 넘길 곳이 없거나 결과가 불명이면 `DEFER`로 큐에 되돌린다(PROTOCOL 7.2, 12.2, 13.4). 무인 깨우기의 확정은 깨운 세션이 `become(wake)`로 받았음을 증명할 때다.
- **수동 수신**이 정식 모드다: 입력 통로가 없는 세션은 `wait_for_message`·`wait_for_reply`(한 번 최대 45초, 호출 사이 90초 임대)로 받는다.
- **기동 조건**: `[wake]` 없이도 리시버가 뜨고, 받을 곳(깨울 러너·바인딩을 쥔 세션)이 있을 때만 서버 자리를 잡는다. `brv init --attended-only`는 리시버 서비스를 등록하되 무인 깨우기만 끈다.
- **`brv listen`**은 리시버 관찰 명령이 됐다 — 받은 메시지의 행선지·수락·깨우기 결과를 보여 주고 아무것도 가져가지 않는다.
- **갱신**: 옛 실행 파일(`.old`) 자동 정리, 갱신 전 중계기는 요청마다 거부되며 이유를 말한다, 이전 리시버가 넘기지 못한 메시지는 `brv status`에 보인다.
- **러너 표기**: `brv status`가 러너마다 무인 깨우기 / CLI 밀어넣기 / GUI 밀어넣기를 실측 여부와 함께 보인다.
- **윈도우 `.cmd` 러너 결함 수정**: 프로필 인자에 `{prompt}`가 없으면 프롬프트를 표준 입력으로 넘긴다(Codex `exec -`). `cmd.exe` 감싸기가 여러 줄 프롬프트를 첫 줄에서 자르던 문제의 근본 수정. `wake show`·`wake test`가 인자 전달이 남은 `.cmd` 설정을 경고한다.
- **원격 MCP** 대기 도구 한 번의 상한·기본이 45초로 바뀐다(호스트가 60초에 끊는 문제).

## 삭제

`brv connect`·`brv connection`·`brv desktop run/resolve/status`·`brv mcp setup`·`brv mcp --binding/--claude-channel/--codex-cli-endpoint`. 대신 세션 안에서 `receiver_connect`·`receiver_resolve`를 쓴다.

## 갱신 절차

- **서버를 먼저** 올린다(이 버전은 서버의 `WORKING`/`DEFER`를 전제로 한다 — 옛 서버에서는 무인 깨우기가 종전처럼 스폰 직후 확정된다).
- 설치 한 줄로 갱신한 뒤 **`brv mcp register`를 다시 실행**한다 — 옛 등록에 남은 `--binding`을 새 중계기가 거부한다. 앱 안에 떠 있던 중계기는 앱의 MCP를 재시작한다.
- Codex 깨우기 설정은 `brv wake set --runner codex`로 다시 저장하면 표준 입력 프로필이 된다(기존 설정도 그대로 동작하지만 윈도우 `.cmd` 심에서는 여러 줄 프롬프트가 잘린다 — `brv wake show`가 경고한다).

## 검증

Windows GNU에서 공개 198건·서버 전체 스위트(실제 통합 36 바이너리) 및 양쪽 fmt/clippy 통과. 실제 모델 왕복: Claude Code 2.1.263 무인 깨우기·수동 수신·Monitor 밀어넣기, Codex 0.153.4 무인 깨우기. 미실기: Channels·Codex queue(대화형), Codex Desktop, 윈도우 서비스 모드, macOS·Linux.

설치: https://brevduva.dev/install.sh 또는 https://brevduva.dev/install.ps1
