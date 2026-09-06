# brv 0.6.31

현재 Codex Desktop 작업에 메시지를 전달하는 실험 기능을 리시버에 추가했습니다.

- `brv connect --binding org/agent@channel`: 현재 작업 셸의 식별자로 연결하고 백그라운드 수신을 시작합니다.
- `brv connection status|pause|resume|disconnect`: 연결 상태와 수신을 관리합니다.
- 로컬 MCP의 `receiver_connect`, `receiver_connection` 도구를 추가했습니다. 새 도구를 사용하려면 MCP 프로세스를 재시작하세요.
- `brv connection restart`: 현재 프로필의 활성 연결을 새 바이너리로 재시작합니다. 설치기에도 적용했습니다.
- 전송 전 로컬 기록, 중복 수신 방지, 작업 중 도착한 메시지의 대기 처리를 추가했습니다.
- 전송 결과가 불명확하면 자동 재전송하지 않습니다. 작업 이력 확인 후 `brv desktop resolve`로 복구합니다.

## 지원·검증 범위

Windows 실제 Codex Desktop 대화 왕복을 확인했습니다. macOS·Linux Unix 소켓 경로를 구현했으며,
이전 Linux WSL 소켓·worker 테스트가 통과했습니다. **macOS·Linux 실제 앱 대화는 미검증이며 배포 후 검증 예정입니다.**
내부 IPC를 사용하는 실험 기능으로, 호환되는 Codex Desktop 실행체가 필요합니다.
임의의 Codex CLI·Claude 기존 세션 연결이나 OS 로그인 자동 시작을 제공하지 않습니다.

Windows 회귀 테스트 88개, fmt·clippy와 설치기 구문 검사를 통과했습니다.
입력 수락은 모델 작업 완료를 의미하지 않습니다.

## 설치

macOS/Linux: `curl -fsSL https://brevduva.dev/install.sh | sh`

Windows: `irm https://brevduva.dev/install.ps1 | iex`

다른 프로필은 해당 `BREVDUVA_CONFIG`로 별도 재시작하세요. 기존 로컬 MCP 프로세스도
재시작해야 새 도구가 반영됩니다. 원격 MCP는 이번 리시버 배포와 별개입니다.

0.6.30의 게시 전 macOS CI에서 발견한 서비스 코드의 OS별 컴파일 조건을 수정했습니다. 0.6.30 릴리스 빌드는 취소하고 0.6.31로 배포합니다.
