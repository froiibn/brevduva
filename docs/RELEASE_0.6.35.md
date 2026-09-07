# brv 0.6.35

Copyright 2026 SEIZIA (Jaeyoung Ko). SPDX-License-Identifier: Apache-2.0

일반 CLI의 자동 수신 활성화 요청이 Desktop용 연결 명령으로 이어지는 오류와 설정 파일 접근 오류 안내를 수정합니다.

- 일반 MCP의 `receiver_connect`에 실제 실행 환경인 `session_kind`를 필수로 받습니다. `codex-cli`·`claude-cli`·기타/미확인 호스트에서는 Desktop용 셸 명령을 만들지 않습니다.
- 일반 CLI는 `automatic_delivery=false`와 현재 실행 환경에서 활성화할 수 없는 이유를 반환합니다. 권한 확대·반복 조회·다른 세션 생성을 자동 수신의 대체 성공으로 안내하지 않습니다.
- `codex-desktop`은 기존 현재 작업 셸 및 실제 Desktop owner 검증을 유지합니다. 공유 app-server 모드의 `receiver_connect(thread_id)`와 Claude Channels 전달 경로도 유지합니다.
- 설정 파일의 접근 거부와 파일 없음을 구분합니다. 접근 거부에 `brv init`으로 기존 설정을 다시 만들도록 권하지 않으며 원래 OS 오류를 보존합니다.
- `brv connect` 도움말과 연결 실패 안내에 Desktop 전용 범위를 명시합니다.

## 남아 있는 요구사항

**일반 `codex`·`claude` 실행과 일반 MCP 등록만으로 현재 대화에 자동 수신을 활성화하는 기능은 아직 미충족입니다.** 이번 릴리스는 잘못된 연결 경로와 진단을 수정한 버전이며, 그 자동 주입 요구를 해결한 버전이 아닙니다. 사용자 환경 시험에서는 특수 실행 옵션이나 수동 수신 조회로 성공 판정을 대신하지 않습니다.

## 검증과 업데이트

Windows·macOS·Linux의 포맷·Clippy·회귀 CI를 통과한 커밋에서 5종 배포 바이너리를 빌드합니다. 일반 CLI의 명령 미생성·미접속, Desktop 명령 유지, 오류 종류별 안내를 회귀셋에 추가했습니다.

기존 설치 명령으로 갱신한 뒤 `brv --version`을 확인하고 로컬 MCP를 재시작해야 새 도구 정의가 적용됩니다. 원격 MCP나 모델이 자체 작성한 셸 명령은 이 로컬 도구의 검사를 거치지 않으므로, 그 경로까지 수정됐다고 보장하지 않습니다.
