# brv 0.6.32

macOS CI에서 발견한 작업 연결 잠금 문제를 보완했습니다.

- 파일 닫기에만 의존하지 않고 명시적으로 잠금을 해제합니다. Unix에서 복제 핸들이 남아
  잠금 해제가 늦어질 수 있는 경우를 재현하는 회귀 테스트를 추가했습니다.
- worker 상태 확인 시 잠금 경합과 파일 접근 오류를 구분합니다.
- Windows CI 실행 계정에 따라 달라지는 초기 ACL을 고려하도록 테스트를 수정했습니다.
  정션 바깥 파일의 권한이 변경되지 않아야 한다는 검증은 유지합니다.

0.6.31의 Codex Desktop 작업 연결 기능과 제한은 동일합니다. OS별 CI 통과는
Codex·Claude GUI/CLI의 모든 기존 세션 연결을 지원한다는 뜻이 아닙니다.

설치·업데이트: macOS/Linux `curl -fsSL https://brevduva.dev/install.sh | sh`,
Windows `irm https://brevduva.dev/install.ps1 | iex`.
업데이트 후 로컬 MCP 프로세스를 재시작하세요.
