# OS별 현재 작업 연결 구현과 검증

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

## 공통 사용자 흐름

이 문서의 IPC와 worker는 Desktop 경로다. v0.6.34의 Codex CLI 공유 app-server
어댑터는 [별도 준비·검증 범위](CODEX_CLI.md)를 따른다. Windows의 실제 TUI와 모의 모델
통합을 확인했으며 macOS/Linux TUI 실기는 아직 미검증이다. Claude CLI는
[Channels](CLAUDE_CHANNEL.md) 경로를 사용한다.

사용자는 현재 에이전트에게 연결을 요청한다. `brv connect`가 현재 작업의 호스트
식별자를 읽고 실제 소유 실행체를 확인한다. 상태·일시정지·재개·해제와 메시지
보관·중복 방지 규칙은 OS에 관계없이 동일하다. 서버와 프로토콜은 변경하지 않는다.

| 구분 | Windows | macOS·Linux |
|---|---|---|
| 로컬 IPC | `\\.\pipe\codex-ipc` | `$CODEX_HOME/ipc/ipc.sock` |
| CODEX_HOME 미지정 | 파이프 경로 고정 | 사용자 홈의 `.codex/ipc/ipc.sock` |
| 실행 | 사용자 계정, 숨긴 콘솔 | 사용자 계정, worker 시작 시 `setsid` |
| 제어 | 파일 기반 연결 의도와 세대 번호 | 동일 |
| 전달 | 실제 소유자에 `thread-follower-start-turn` | 동일 |

독립된 CLI의 CODEX_THREAD_ID가 있다는 사실만으로 연결하지 않는다. 이 정확한
작업을 소유하고 외부 입력을 지원하는 실행체가 없으면 명시적으로 실패한다.
Linux용 Desktop 제품의 설치 가능성·배포 유무를 이 구현으로 보장하지 않는다.

## 경로의 코드 근거

조사한 설치 번들의 `.vite/build/src-VqXTPopo.js`에서 `f9()`는 Windows 파이프 또는
`iA()/ipc/ipc.sock`을 반환한다. `iA()`는 `CODEX_HOME`, 없으면 사용자 홈의 `.codex`다.
같은 코드가 4바이트 little-endian 길이와 JSON 프레임을 사용한다. 이전 temp 경로도
번들에 존재하지만 이번 구현은 현재 primary 경로만 사용한다. 다른 사용자의 소켓이나
최근 실행체를 탐색하지 않고, 로컬 설정 또는 호스트 기본 경로만 사용한다.

Unix 연결에서는 디렉터리와 소켓의 사용자 소유권, 디렉터리의 group/world 쓰기
금지, 실제 소켓 타입을 검사한다. 연결 후 peer credential도 현재 UID와 비교한다.
소켓 파일을 지우거나 Desktop의 디렉터리 권한을 수정하지 않는다.

worker는 `rustix`의 안전한 API로 `setsid`를 호출한다. 기존 프로세스의 권한이나
터미널 설정을 바꾸지 않으며, 준비가 확인된 후 connect가 반환한다. 현재 사용자
계정으로만 실행하고 OS 로그인 자동 시작 설치는 별도 후속 범위다.

## 검증 결과 (2026-09-07)

- Windows: 기존 85개 테스트와 fmt/clippy 통과. 실제 Desktop 왕복 및 busy 후
  자동 전달 결과는 [기존 기록](DESKTOP_RECEIVER.md) 참조.
- Linux: Ubuntu 24.04 WSL에서 프로젝트 고정 Rust 1.98.0으로 전체 테스트 통과.
  Unix 경로 선택, 실제 Unix 소켓의 프레임/owner/turn 요청, 쓰기 가능한 디렉터리·
  심볼릭 링크 거부, 오래된 소켓의 비파괴 실패를 포함한다.
- Linux worker 통합 테스트: 실제 `brv connection worker` 프로세스를 시작해
  SID=PID(터미널 분리), Desktop 없는 상태의 `waiting_for_app`, 연결 해제 후
  정상 종료를 확인했다. 서버 토큰과 실제 사용자 세션을 사용하지 않았다.
- Linux 총 86개 테스트 통과. Windows 전용 테스트와 Unix 전용 테스트 구성이 달라
  총 개수는 OS별로 다르다. Linux 모의 owner 검증을 실제 모델 응답 검증으로 부르지 않는다.
- macOS: Unix 코드를 포함했고 CI 매트릭스에 macos-latest를 추가했다. 현재 머신에는
  macOS 실행 환경이 없어 실제 실행은 미검증이며 CI도 아직 실행하지 않았다.

Windows 호스트의 Linux musl/macOS 교차 검사는 C 컴파일러 부족으로 중단됐다.
Linux는 WSL 네이티브 빌드·실행으로 대신 검증했다. macOS 교차 검사 성공을 주장하지
않는다. WSL 검증 도구와 산출물은 `/tmp/brv-platform-tools`, `/tmp/brv-platform-target`에
격리했으며 사용자 기본 Rust 설정과 셸 PATH는 변경하지 않았다.

## 배포 전 남은 확인

macOS CI 및 실제 앱 연결, Linux의 호환 실행체와 실제 대화 왕복, 각 OS에서
터미널을 닫은 뒤 수신 지속, 재로그인·OS 자동 시작, CLI와 Claude 기존 세션 어댑터는
아직 추가 검증 또는 구현 대상이다. 같은 소켓을 쓴다고 모든 실행체가 지원되는 것은 아니다.

## macOS 잠금 실패 후속 수정

최종 CI의 잠금 해제 직후 alive 판정 실패를 조사하여, 파일 닫기에만 의존한 잠금을
명시적 unlock을 수행하는 FileLock 가드로 변경했다. Unix의 복제된 파일 디스크립터는
같은 open-file-description을 공유하므로 fork 중 다른 핸들이 남으면 close만으로는
잠금이 즉시 해제되지 않을 수 있다. 실제 실패 실행에서 상속된 핸들을 추적한 것은
아니므로 해당 실행의 원인을 확정하지는 않는다.

회귀 테스트는 try_clone으로 이 잠금 수명 문제를 재현한다. 원래 File을 닫아도 복제
핸들이 남으면 잠금이 유지됨을 확인하고, 새 가드를 해제하면 복제 핸들이 남아 있어도
잠금이 해제됨을 검증한다. connection의 control/worker와 desktop 저널 잠금에 동일하게
적용한다. 상태 확인은 WouldBlock만 실행 중으로 판단하고 파일 접근 오류는 오류로
반환한다. 기존 macOS 실패 테스트의 즉시 해제 검사는 약화하거나 삭제하지 않았다.
