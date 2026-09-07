# brv 0.6.36

일반 실행한 Codex·Claude Code CLI에서 자동 수신 활성화 요청이 실제 현재 대화의 전달
경로를 만들도록 수정합니다. v0.6.35의 진단·차단 수정에서 실제 전달 구현으로 확장합니다.

- Codex CLI: 현재 작업의 고유 queue로 유휴 대화에 수신 안내를 넣습니다.
- Claude Code CLI: 현재 대화의 고유 Monitor에 수신 스트림을 연결합니다. Monitor가 제공되는 호스트가 필요합니다.
- 외부 본문은 receipt 도구 결과로 전달합니다. 큐 등록·모델 관측·업무 완료를 구분합니다.
- 연결 실패 시 미확정 전달을 복구 가능한 상태로 보존하고 같은 세션의 늦은 receipt를 처리합니다.
- 로컬 MCP에 새 도구를 반영하려면 업데이트 후 MCP를 재시작해야 합니다. 전용 CLI 시작 옵션은 필요하지 않습니다.

Windows 실제 Codex 0.153.4·Claude Code 2.1.263 일반 TUI와 로컬 모의 모델/WS에서 전달
경로를 검증했습니다. 3개 OS의 CI 및 5종 배포 바이너리 빌드는 실제 모든 OS·GUI의
종단 검증을 뜻하지 않습니다. macOS/Linux 실제 CLI, Monitor 없는 Claude, 임의의 GUI/웹
호스트는 전체 목표의 남은 검증·개발 범위입니다.

[설계·재현 시험](NATIVE_SESSION_DELIVERY.md).
