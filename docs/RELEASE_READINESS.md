# 현재 작업 연결 기능의 정식 배포 준비 점검

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

점검일: 2026-09-07. 대상: 현재 미커밋 변경. 웹 Docs 구축은 후순위로 둔다.

## 판정

Windows 실험 기능의 동작 근거는 있으나, macOS·Linux를 포함한 정식 지원으로
배포할 준비는 아직 완료되지 않았다. 점검 후 아래 1~3번의 코드를 보완했다.
태그·배포는 하지 않았다.

## 배포 전 보완

후속 반영: `connection restart`와 설치기 호출로 현재 프로필의 connected worker를
새 바이너리로 재기동한다. paused/disconnected는 유지한다. 앱이 닫혀 있으면 대기 상태로
재시작하며, 불명확한 전달은 재실행하지 않는다. 다른 설정 파일을 쓰는 프로필은 해당
`BREVDUVA_CONFIG`로 별도 재시작해야 한다. MCP 프로세스 재시작 안내도 추가했다.

`desktop resolve`는 정지된 연결의 불명확한 메시지에만 적용하며, 확인 근거와 명시적인
확정이 필요하다. `connect`는 실제 receiving/standby 상태를 확인하고 시작 실패·오류·
10초 내 준비되지 않음을 실패로 반환한다. timeout 이후 worker가 계속 준비 중일 수
있다는 안내를 포함한다. 아래는 최초 발견 사항이며, 실제 업데이트 왕복 검증은 남아 있다.

1. **업데이트 시 작업 연결 worker 처리**
   `install.ps1`과 `install.sh`는 바이너리를 교체한 뒤 `daemon restart`만 실행한다.
   `connection.rs`의 별도 worker는 이 서비스의 관리 대상이 아니다. 실행 중인 worker가
   이전 바이너리로 남을 수 있고, 같은 작업에 다시 connect해도 살아 있는 worker를 그대로
   사용한다. worker의 안전한 종료·새 버전 재기동 및 보관된 메시지 유지 검증이 필요하다.
   MCP 호스트의 기존 프로세스에도 새 도구가 반영되도록 재시작 안내가 필요하다.

2. **불명확한 전달 결과의 복구 경로**
   `desktop.rs`는 IPC 응답 유실을 unknown으로 기록하고, submitting/unknown이 있으면
   재시작을 거부한다. 중복 실행 방지 정책은 유지해야 한다. 그러나 현재 CLI에는 실제 작업
   이력을 확인한 운영자가 결과를 확정하는 명령이 없다. pause/resume/disconnect로도
   이 상태는 해소되지 않는다. 저널 직접 편집·삭제에 의존하지 않는 복구 절차가 필요하다.

3. **connect 성공 판정**
   `connection.rs::command`는 worker 잠금을 최대 2초 기다린 뒤 상태를 출력하고 성공을
   반환한다. worker가 시작하지 못했거나 토큰 오류로 곧바로 종료되어도 명령 종료 코드는
   성공일 수 있다. 시작 실패를 오류로 반환하고, 시작 중과 실제 수신 준비를 구분하는
   검증이 필요하다.

4. **플랫폼 및 배포 산출물 검증**
   macOS 실제 실행과 Linux 호환 Desktop의 실제 모델 왕복은 미검증이다. 추가한 3개 OS
   CI도 아직 실행하지 않았다. Linux WSL 테스트를 macOS 또는 Linux 앱 연결 검증으로
   대체할 수 없다. 정식 지원 범위와 실제 검증 범위를 일치시켜야 한다.
   배포 대상 5개 바이너리의 release 빌드 및 설치 검증도 필요하다.

## 릴리스 준비 항목

- 현재 버전은 여전히 0.6.29다. 새 버전·변경 기록·배포할 파일 목록을 확정해야 한다.
- 신규 구현과 docs/scripts는 아직 Git 미추적 상태다. PoC와 실제 테스트 식별자가 있는
  조사 기록을 제품 문서와 함께 공개할지 검토한 후 명시적으로 커밋 대상을 고른다.
- release workflow는 빌드 후 게시하며 CI 테스트를 직접 선행 조건으로 두지 않는다.
  태그 전에 해당 커밋의 테스트 통과를 확인하는 절차가 필요하다.
- CLI·Claude의 임의 기존 세션 연결, OS 로그인 자동 시작은 구현 범위 밖이다.
  자동 깨우기는 호환 Codex Desktop 작업 연결로 명시한다.
- 다중 연결 MCP의 선택 UX는 별도 미완성 항목이다. 현재 단일 대상을 선택해야 하는
  제약을 유지하며, 계정 선택 UI가 제공되는 것처럼 안내하지 않는다.

## 검증 근거

- 이번 점검: Windows에서 `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace --locked --quiet`
  모두 통과. 보완 후 전체 88개 테스트, 실패 0개. `git diff --check` 통과.
  설치기 PowerShell 구문 분석 및 WSL `sh -n` 통과. 실제 다운로드/설치는 실행하지 않았다.
- 이전 검증: Windows 실제 Desktop 왕복·busy 대기·pending 재시작 복구.
  [기록](DESKTOP_RECEIVER.md) 참조. 이번 점검에서 실제 메시지를 재전송하지 않았다.
- 이전 검증: Linux WSL 전체 86개 테스트 및 Unix 소켓·worker 분리 테스트.
  [플랫폼 기록](PLATFORM_CONNECTION.md) 참조. 이번 점검에서 Linux 테스트를 재실행하지 않았다.
- 현재 점검은 로컬 코드 기준이며, 배포된 서비스와 설치 패키지가 이 변경을 포함한다는
  의미가 아니다.

## 배포 진행 결정

사용자 요청에 따라 실제 macOS·Linux 앱 검증은 배포 후 수행한다.
0.6.30 후보의 macOS CI에서 기존 service.rs의 미사용 함수 경고가 발견되어 게시를 취소했다.
OS별 컴파일 조건을 수정한 c0f5351(v0.6.31)로 배포를 진행한다.
로컬 Windows 88개 테스트와 fmt/clippy가 통과했다. PoC scripts와 초기 조사 문서는 이번 커밋에 포함하지 않았다.

게시 확인: v0.6.31은 5개 플랫폼 자산과 SHA256SUMS로 공개됐으며 latest도 v0.6.31이다.
웹 install.sh/install.ps1을 갱신하고 로컬 파일과 SHA256을 대조했다.
Windows 배포 zip의 체크섬을 확인하고 추출된 바이너리에서 brv 0.6.31을 확인했다.
macOS·Linux CI는 릴리스 커밋에서 통과했다. Windows CI의 기존 ACL 준비 조건을
테스트 전용 커밋 a3f1605에서 수정했다(제품 동작 코드 변경 없음). 로컬 전체 88개 테스트 통과.

잠금 수정 결과: 79957a8에서 Windows/macOS/Linux CI 전부 통과.
https://github.com/froiibn/brevduva/actions/runs/34050333546
Windows 89개, macOS/Linux 91개 테스트. 기존 실패 검사와 복제 핸들 재현 검사 유지·통과.
이 커밋을 v0.6.32로 배포한다.
