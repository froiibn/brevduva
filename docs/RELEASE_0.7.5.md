# brv 0.7.5

Copyright 2026 SEIZIA (Jaeyoung Ko)

윈도우 실행 파일에 코드 서명이 붙는다 — 게시자 "Seizia". 깨우기가 실패하면 원인 줄이 상태 화면에 바로 보인다. 설계 이력은 [RECEIVER_DESIGN.md](RECEIVER_DESIGN.md) P5(2026-09-23).

## 바뀐 것

- **Windows: `brv.exe`가 서명된다.** Azure Artifact Signing으로 Authenticode 서명하고 타임스탬프를 붙인다. 파일 속성의 디지털 서명과 관리자 승격 창에 게시자가 "알 수 없는 게시자"가 아니라 **Seizia**로 나온다. 서명 인증서는 매일 새로 발급되는 짧은 인증서라 타임스탬프가 서명을 계속 유효하게 유지한다. 게시자가 새로 생긴 이름이라 SmartScreen 경고는 처음 얼마간 남을 수 있다 — 평판은 다운로드가 쌓이면서 생긴다.
- **깨우기 실패의 원인이 보인다.** 사전 점검이든 운영 중 깨우기든 실패하면 `brv status`의 `WAKE UNAVAILABLE` 이유, 데몬 로그, `brv wake test`의 실패 문장에 러너가 `wake.log`에 남긴 원인 줄이 붙는다(`last output in wake.log: …`). 시간 초과로 강제 종료한 경우도 같다 — 러너가 API 오류를 재시도하다 상한에 걸리면 그 오류 줄이 나온다. 종전에는 "exceeded timeout"만 나와 원인을 보려면 로그 파일을 열어야 했다. 다른 에이전트에게 가는 실패 보고에는 이 줄을 싣지 않는다 — 러너 출력은 이 머신의 경로와 환경을 담을 수 있다.

## 갱신 절차

설치 한 줄뿐이다. 이미 설치된 윈도우 PC는 종전처럼 서비스가 관리자 승격 없이 새 실행 파일로 재기동된다 — 그래서 갱신할 때는 승격 창이 뜨지 않는다. 서명은 설치된 `brv.exe`의 속성 → 디지털 서명 탭에서 확인할 수 있다. 처음 설치하는 PC는 `brv daemon install`의 승격 창에 게시자 Seizia가 나온다.

갱신 뒤에는 종전처럼 실행 중인 AI 앱·CLI의 로컬 MCP를 재시작한다.

## 검증

공개 시험·fmt/clippy는 Ubuntu·Windows·macOS CI에서 통과(깨우기 실패 발췌 회귀 시험 2건 추가). 서명 경로는 공개 없는 시험 워크플로로 먼저 실측했다 — 서명 상태 Valid, 서명자 `CN=Seizia, O=Seizia, L=Cheongju-si, S=North Chungcheong, C=KR`, 발급자 Microsoft ID Verified CS AOC CA 04, Microsoft 공개 타임스탬프. 릴리스·시험 워크플로는 actionlint를 통과했다.

설치: https://brevduva.dev/install.sh 또는 https://brevduva.dev/install.ps1
