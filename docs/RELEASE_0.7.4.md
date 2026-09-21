# brv 0.7.4

Copyright 2026 SEIZIA (Jaeyoung Ko)

macOS 배포를 Apple의 정식 경로에 맞췄다 — Developer ID 서명·공증, 앱 묶음 `Brevduva.app`, macOS의 서비스 등록 API(SMAppService). 윈도우 실행 파일에는 아이콘과 버전 정보가 들어간다. 설계 이력은 [RECEIVER_DESIGN.md](RECEIVER_DESIGN.md) P8(2026-09-22), 맥북 실측은 2026-09-21~22.

## 바뀐 것

- **macOS: "확인되지 않은 개발자" 경고가 없어진다.** 릴리스 자산을 Developer ID로 서명하고 Apple 공증을 받는다. 앱 묶음에는 공증 증명이 첨부돼(staple) 오프라인에서도 검증된다.
- **macOS: 리시버가 앱 묶음 `Brevduva.app`으로 설치된다.** 위치는 `~/Library/Application Support/brevduva/`(`BRV_APP_DIR`로 변경 가능), `~/.local/bin/brv`는 그 안의 실행 파일을 가리키는 심볼릭 링크다 — 터미널에서 쓰는 명령은 그대로 `brv`다. 서비스는 앱 안의 정의를 SMAppService로 등록한다(여전히 launchd LaunchAgent다). 이유: 서명한 단독 실행 파일을 `~/Library/LaunchAgents`의 plist로 등록하면 시스템 설정의 **로그인 항목 → 앱 백그라운드 활동**에 프로그램 이름이 아니라 인증서의 개발자 이름이 나온다 — 묶음 안에 넣어도, plist에 `AssociatedBundleIdentifiers`를 넣어도, `~/Applications`로 옮겨도 같았고, SMAppService 등록만 "Brevduva"로 유지됐다(실측). 개인정보 보호 목록에도 "Brevduva"와 아이콘으로 나온다. macOS 13 미만과 직접 빌드한 단독 실행 파일은 종전대로 plist로 등록한다.
- **macOS: 토큰이 키체인으로 옮겨진다.** 서명된 빌드는 고정된 코드 정체성을 가지므로 토큰 주 저장소가 파일에서 키체인으로 바뀐다(애드혹 서명 시절에는 "항상 허용"이 유지되지 않아 파일을 썼다). 옮긴 뒤 되읽어 확인하고서야 파일을 지운다. **결함 수정**: 같은 에이전트의 옛 형식 토큰 파일(`token-<agent>`)이 현행 파일(`token-<org>-<agent>`)과 다른 값으로 함께 남아 있으면, 두 번째 읽기에서 옛 토큰이 키체인 항목을 덮어써 연결이 끊기고 올바른 토큰이 어디에도 남지 않을 수 있었다 — 이전이 성공하면 그 바인딩의 토큰 파일을 두 이름 모두 지운다. 서명 빌드의 첫 실기 직전에 발견했고 회귀 시험을 추가했다.
- **macOS: 갱신이 실행 중인 리시버를 강제 종료시키지 않는다.** 종전 설치기는 실행 중인 파일을 제자리에서 덮어써 macOS가 그 프로세스를 죽였다(`OS_REASON_CODESIGNING`, launchd가 곧바로 다시 띄웠다). 이제 앱을 옆에 다 놓은 뒤 이름만 바꿔 교체한다 — 돌고 있는 리시버는 정상 재기동된다.
- **Windows: `brv.exe`에 아이콘과 버전 정보.** 탐색기·작업 관리자·서비스 속성에 Brevduva 아이콘과 제품명·회사·버전이 나온다.

## 갱신 절차

설치 한 줄뿐이다. macOS에서 0.7.3 이하(단독 실행 파일)를 쓰던 머신은 그 한 줄이 앱을 설치하고, `~/.local/bin/brv`를 링크로 바꾸고, 설치기가 부르는 `brv daemon restart`가 서비스 등록을 앱 쪽으로 옮긴다 — 옛 등록의 PATH와 프로필(`--config`)을 그대로 이어받고 옛 `~/Library/LaunchAgents/dev.brevduva.brv-daemon.plist`를 지운다(새 등록이 실패하면 옛 등록을 되살린다). 손으로 할 일은 없다.

macOS에서 알아 둘 것:

- 첫 갱신 때 **폴더 접근 허용 창이 한 번** 뜰 수 있다 — 깨우기 대상 폴더가 데스크탑·문서 등 보호된 위치이고 리시버의 서명 정체성이 바뀌었기 때문이다. 허용하면 된다. 사람이 없는 상태에서 갱신하면 그 창에서 깨우기가 기다릴 수 있다. 서명 빌드에서는 이 동의가 개발자 정체성에 묶이므로 이후 갱신에서는 다시 묻지 않을 것으로 본다(서명판 사이의 교체에서 키체인·폴더 창이 뜨지 않는 것은 실측했다).
- 키체인 허용 창은 실측에서 한 번도 뜨지 않았다(리시버가 자기가 만든 항목에 접근한다).
- 갱신 뒤에는 종전처럼 실행 중인 AI 앱·CLI의 로컬 MCP를 재시작한다.

## 검증

공개 시험·fmt/clippy는 Ubuntu·Windows·macOS CI에서 통과(토큰 파일 정리, 앱 묶음 판별·등록 도구 출력 해석·옛 plist 값 승계·표지 파일 회귀 시험 추가). macOS 실기(arm64, macOS 26.6): Safari로 받은 서명판의 Gatekeeper 통과, 토큰 파일 3개의 키체인 이전, `daemon install`·`restart`·`uninstall`, 옛 plist 등록에서 `brv daemon restart` 한 번으로의 이전, 설치 한 줄 시나리오(지금 상태 위 갱신 / 0.7.3 단독 설치 상태에서 갱신 / 재실행 멱등), 시스템 설정 표시 "Brevduva". 미검증: 백그라운드 항목 토글 끄기·켜기, Intel 맥.

릴리스 뒤 확인(2026-09-22): 공식 한 줄로 설치한 0.7.4 맥북을 재부팅한 뒤 로그인하자 리시버가 백그라운드에서 스스로 떠서 정상 동작했다 — 릴리스 시점에 미검증이던 "재부팅 뒤 등록 유지"가 확인됐다(사용자 보고).

설치: https://brevduva.dev/install.sh 또는 https://brevduva.dev/install.ps1
