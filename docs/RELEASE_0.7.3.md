# brv 0.7.3

Copyright 2026 SEIZIA (Jaeyoung Ko)

윈도우 실기(2026-09-14)에서 드러난 갱신 결함의 수정과 실측 표기 갱신. 설계 이력은 [RECEIVER_DESIGN.md](RECEIVER_DESIGN.md) P8 ⑨·§3.

## 바뀐 것

- **갱신이 서비스에 닿지 못하던 경우를 고쳤다.** 설치기가 서비스 파일을 새 버전으로 바꿀 때 옛 파일을 `brv.old`로 비켜 두는데, 직전 갱신이 비켜 둔 `brv.old`를 앱 안에 남아 있던 옛 중계기가 아직 쥐고 있으면 이름 바꾸기가 거부돼 서비스가 옛 버전으로 남았다(0.7.2 갱신 실사고 — 새 중계기는 "버전이 다르다"며 거부됨). 이제 그럴 때 `brv.old.<시각>`으로 비켜 두고, 그런 잔재도 다음 기동·재기동 때 치운다.
- **옛 중계기가 스스로 물러난다.** 앱 안에 떠 있던 `brv mcp`는 리시버가 새 버전으로 재기동한 것을 보면(5초 안에), 또는 요청이 버전 불일치로 거부되면, 이유를 남기고 종료한다 — 갱신 뒤 작업 관리자에 옛 `brv.exe(.old)` 프로세스가 남지 않는다. 앱에는 MCP 서버 종료로 보이며, 그 앱의 MCP를 재시작하면 현재 중계기가 뜬다.
- **중계기의 버전 불일치 안내를 고쳤다.** 리시버 쪽이 낡았을 때는 세션 재시작이 소용없다 — "`brv daemon restart`로 갱신을 마저 하라"고 말한다.
- **실측 표기**: Codex의 codex queue·Codex Desktop 통로가 실측(measured)으로 바뀐다 — 윈도우 운영 서비스에서 실제 Codex CLI 작업·Codex Desktop 작업·Claude Code GUI/CLI로 전달→receipt→reply 왕복을 확인했다.

## 갱신 절차

설치 한 줄뿐이다. 0.7.2 갱신 뒤 서비스가 0.7.1로 남은 머신(`brv status`가 중계기 버전 불일치를 보이거나 세션의 MCP가 "receiver service still runs 0.7.1"이라고 하는 경우)은 이 버전 설치가 서비스까지 교체한다 — 실패하면 앱을 닫아 옛 중계기를 끝낸 뒤 `brv daemon restart`.

## 검증

Windows GNU에서 공개 시험·fmt/clippy 통과(비켜 두기 이름 충돌·잔재 청소 회귀 시험 추가). 실제 앱 왕복은 위 실측.

설치: https://brevduva.dev/install.sh 또는 https://brevduva.dev/install.ps1
