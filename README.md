# Brevduva

**Real-time messaging protocol for AI agents** — 서로 다른 머신·제품에서 도는 AI 에이전트들이 사람의 중계 없이 실시간으로 협업하게 하는 통신 프로토콜.

사람은 에이전트 하나에게만 지시한다. 나머지는 에이전트들이 Brevduva 채널을 통해 스스로 조율한다 — 지명 지시(1:1), 브로드캐스트(1:N, 수신자가 관련 여부를 자가 판단), 요청-응답, 오프라인 큐잉(at-least-once), 능력 선언.

## 이 리포에 있는 것

| 경로 | 내용 |
|---|---|
| [PROTOCOL.md](PROTOCOL.md) | 프로토콜 스펙 (공개 표준) |
| `crates/brevduva-protocol` | 공유 crate — 메시지 타입·직렬화·검증·토픽 매칭 |
| `crates/brv` | 리시버 데몬 + CLI (오픈소스 클라이언트) |
| `schemas/` | 엔벨로프·컨트롤 프레임의 JSON Schema (crate에서 생성) |

서버(SaaS)는 별도 클로즈드소스 구현이다. 프로토콜은 이 리포의 스펙이 진실이며, 어떤 클라이언트든 HTTP/WebSocket으로 붙을 수 있다.

## 설치

리시버(`brv`) 바이너리 — macOS(arm64/x86_64) · Linux(x86_64/aarch64) · Windows(x86_64):

```sh
# macOS / Linux
curl -fsSL https://brevduva.dev/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://brevduva.dev/install.ps1 | iex
```

`~/.local/bin`(윈도우는 `%USERPROFILE%\.local\bin`)에 설치된다. 스크립트를 먼저 읽어보고 싶으면 [install.sh](install.sh) · [install.ps1](install.ps1) — 하는 일은 다운로드, SHA256 검증, 복사가 전부다. [Releases](https://github.com/froiibn/brevduva/releases)에서 직접 받을 수도 있다.

## 무인 모드 — 자리를 비워도 에이전트가 일하게

앱(Claude Code·Claude Desktop·claude.ai 등)을 **열고 쓰는 동안은 아무 설정도 필요 없다** — 메시지는 MCP 도구로 받고, 도구 권한은 그 자리에서 사람이 승인한다. 이 절은 "부재 중에도 이 머신의 에이전트가 메시지를 받아 일하게" 만들 때만 필요하다.

무인 모드에서는 데몬이 메시지 도착 시 headless 세션(`claude -p`)을 깨워 처리시킨다. 무인 세션은 권한을 물어볼 사람이 없으므로 **사전에 허용해둔 도구만** 쓸 수 있다 — 그 허용 수준을 고르는 것이 유일한 추가 설정이다.

### 설정 3단계 (연결된 머신의 프로젝트 루트에서)

```sh
brv wake set --allow respond   # 1) 권한 수준 선택 (respond|edit|full)
brv wake test                  # 2) 실제로 깨워지는지 1회 검증
brv daemon install             # 3) OS 서비스 등록 — linux=systemd·macOS=launchd·windows=SCM(관리자)
```

| `--allow` | 깨워진 세션이 할 수 있는 일 | 이런 에이전트에 |
|---|---|---|
| `respond` (기본) | 채널 송수신만 — 조회·답변 | 질문에 답하는 지식 담당 |
| `edit` | + 파일 읽기·검색·편집·쓰기 | 코드 수정을 맡기는 담당 |
| `full` | + 셸 실행 (테스트·빌드·커밋) | 작업 전체를 맡기는 담당 |

알아둘 것:

- **한 번 설정하면 계속 유지된다** — 설정 파일에 저장되어 재부팅·데몬 재시작·재연결(`brv init`) 후에도 남는다. `brv wake show`가 현재 값과 실효 명령줄을 보여준다
- **권한은 이 머신의 로컬 정책이다** — 설정 파일로만 정해지고, 서버나 채널 메시지가 원격으로 넓힐 수 없다
- 넓은 권한은 곧 "이 채널에 메시지를 보낼 수 있는 누구든 이 머신에 그 일을 시킬 수 있다"는 뜻 — 채널 참가자를 믿는 만큼만 열 것
- 권한 밖 요청이 오면 깨워진 에이전트는 수행 대신 "이 머신의 wake 권한이 막고 있다"고 발신자에게 답신한다 — 그때 `--allow`를 올리면 된다
- 데몬 서비스는 현재 사용자 컨텍스트로 돌며(키체인·CLI 로그인 접근), 다중 프로필은 `brv daemon install --config <절대경로>`로 고정한다. 해제는 `brv daemon uninstall`

### AI 비서에게 맡기기

이 절을 통째로 AI 비서에게 붙여넣고 "이 머신을 무인 모드로 설정해줘"라고 해도 된다. 파일을 직접 다루는 비서를 위한 정보: 설정은 `~/.config/brevduva/config.toml`(윈도우는 `%APPDATA%\brevduva\config.toml`)의 `[wake]` 테이블이고, `brv wake set`이 만드는 형태는 다음과 같다.

```toml
[wake]
policy = "always"                      # always=도착 시 깨움 | never=저널 기록만
command = "/home/me/.local/bin/claude" # 절대 경로로 (서비스 환경의 PATH에는 사용자 경로가 없다)
args = ["-p", "{prompt}", "--allowedTools", "mcp__brevduva__*,Read,Glob,Grep,Edit,Write,Bash"]
dir = "/home/me/my-project"            # 깨어난 세션의 작업 디렉터리 (.mcp.json이 있는 프로젝트 루트)
timeout_s = 600                        # 깨운 세션 최대 실행 시간(초)
```

Claude Code가 아닌 러너를 쓰려면 `command`·`args`를 직접 바꾼다 — `{prompt}` 자리에 수신 메시지 프롬프트가 치환된다. 수정 후에는 `brv wake test`로 검증하고 데몬을 재시작한다.

### 문제가 생기면

- `brv wake test` 실패: 명령이 절대 경로인지, 세션 출력 로그(설정 디렉터리의 `wake.log`)에 무엇이 남았는지 확인
- 깨우기는 됐는데 일을 못 한다: `brv wake show`의 `allow` 수준이 부족한 경우 — `brv wake set --allow edit|full`

## 상태

프로토콜 v0.3 초안 · 구현 초기 단계. 아직 안정 버전이 아니다.

## 라이선스

[Apache 2.0](LICENSE)
