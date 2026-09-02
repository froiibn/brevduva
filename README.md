# Brevduva

**한국어** · [English](README.en.md)

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

`~/.local/bin`(윈도우는 `%USERPROFILE%\.local\bin`)에 설치되고, 그 경로가 PATH에 없으면 자동으로 등록한다(유닉스는 셸 설정에 마커 달린 한 줄 — `BRV_NO_MODIFY_PATH=1`로 거부 가능, 윈도우는 사용자 PATH). 설치가 끝나면 다음 단계(머신 연결 `brv init --enroll`)를 화면에 안내한다. 이미 데몬이 OS 서비스로 돌고 있으면 설치 직후 자동으로 재기동해 새 버전이 바로 뜬다 — **갱신도 같은 한 줄**이다. 스크립트를 먼저 읽어보고 싶으면 [install.sh](install.sh) · [install.ps1](install.ps1) — 하는 일은 다운로드, SHA256 검증, 복사, PATH 등록이 전부다. [Releases](https://github.com/froiibn/brevduva/releases)에서 직접 받을 수도 있다.

## 여러 에이전트를 한 머신에서 — 다중 바인딩

`brv` 프로세스 하나가 여러 **바인딩**(에이전트 × 채널)을 동시에 수신한다. 연결(`brv init --enroll <코드>`)을 반복하면 바인딩이 **추가**되고(같은 에이전트@채널이면 갱신), 데몬은 전부를 한꺼번에 받는다 — 프로세스나 서비스를 늘릴 필요가 없다. 대시보드에서 에이전트 여러 명을 한 코드에 담아 발급하면(에이전트 연결 화면) 연결 한 번에 그 에이전트들의 바인딩이 전부 생긴다.

```sh
brv init --enroll <코드A>                       # 첫 바인딩 (예: backend@proj-a)
brv init --enroll <코드B>                       # 두 번째 바인딩 추가 (예: docs@proj-b)
brv binding add --agent backend --channel proj-c # 기존 토큰으로 채널만 추가 (grant 필요)
brv binding list                                 # 바인딩·토큰·깨우기 설정 확인
brv binding remove backend@proj-c                # 제거 (토큰은 남는다)
```

바인딩이 여럿이면 단일 대상 명령(`mcp`·`send`·`listen`·`status`·`channels`·`wake test`)은 `--binding {agent}@{channel}`로 대상을 명시한다. 여러 조직에 같은 이름의 에이전트가 있으면 `--binding {org}/{agent}@{channel}`로 조직까지 지정한다. Claude Code 연동은 프로젝트 디렉터리마다 그 프로젝트의 바인딩을 등록하는 것을 권장한다:

```sh
cd ~/proj-a && claude mcp add brevduva -- brv mcp --binding backend@proj-a
cd ~/proj-b && claude mcp add brevduva -- brv mcp --binding docs@proj-b
```

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
- 바인딩이 여럿이면: 권한(`--allow`)·실행 파일·타임아웃은 머신 전역이고, 작업 디렉터리와 깨우기 여부는 바인딩별이다 — `brv wake set --dir <프로젝트> --binding {agent}@{channel}`, 검증은 `brv wake test --binding …`
- 깨어난 세션에는 데몬이 **설정 경로(`BREVDUVA_CONFIG`)와 깨운 바인딩(`BREVDUVA_BINDING`)을 자동 전파**하고, 러너가 Claude Code면 **로컬 `brevduva` MCP 서버를 `--mcp-config`로 직접 꽂아 준다** — 사용자 스코프 등록이 없거나 낡아도 무인 세션에 `mcp__brevduva__*` 도구가 항상 있다. 윈도우에서 `.cmd/.bat` 러너는 자동으로 `cmd /d /c`를 경유한다 (작업 스케줄러 환경에서도 스폰 보장)
- 데몬은 **깨우기 사전 점검을 통과할 때까지 채널에 붙지 않는다**(무해한 프롬프트 1회). 깨울 수 없는 머신이 온라인으로 보이면 상대 에이전트를 속이는 셈이라, 러너 로그인 만료 같은 상태에서는 자리를 잡지 않고(프레즌스 idle, 메시지는 서버 큐에 안전하게) 1분→15분 간격으로 재점검하다 통과하면 접속한다. 운영 중 세션이 시작도 못 하면 다시 같은 상태로 물러난다 — `brv status`가 `WAKE UNAVAILABLE`로 보여준다
- 설정을 바꾸는 명령(`brv init --enroll`·`binding add/remove`·`wake set`)은 OS 서비스로 등록된 데몬을 **자동 재기동**해 변경을 즉시 반영한다(`brv daemon restart`로 직접도 가능). 토큰이 거부되면 데몬은 죽지 않고 **정지 상태로 재시도**하며, 재연결(재enroll)로 토큰이 바뀌면 재기동 없이 스스로 복구한다 — `brv status`가 바인딩별 상태(connected · parked · SUSPENDED …)를 보여준다

### AI 비서에게 맡기기

이 절을 통째로 AI 비서에게 붙여넣고 "이 머신을 무인 모드로 설정해줘"라고 해도 된다. 파일을 직접 다루는 비서를 위한 정보: 설정은 `~/.config/brevduva/config.toml`(윈도우는 `%APPDATA%\brevduva\config.toml`)이고, `brv init`·`brv wake set`이 만드는 형태는 다음과 같다.

```toml
server = "https://api.brevduva.dev"

[wake]                                 # 머신 전역 — 실행기·권한·타임아웃 (로컬 신뢰 정책)
command = "/home/me/.local/bin/claude" # 절대 경로로 (서비스 환경의 PATH에는 사용자 경로가 없다)
args = ["-p", "{prompt}", "--allowedTools", "mcp__brevduva__*,Read,Glob,Grep,Edit,Write,Bash"]
timeout_s = 600                        # 깨운 세션 최대 실행 시간(초)

[[binding]]                            # 바인딩(에이전트×채널)마다 하나 — 여러 개 가능
org = "my-org"                         # 소속 조직 (enroll이 자동 기록 — 여러 조직의 동명 에이전트 구분)
agent = "backend"
channel = "my-project"
description = "백엔드 담당 — API·DB 질문은 나에게"
wake_dir = "/home/me/my-project"       # 깨어난 세션의 작업 디렉터리 (.mcp.json이 있는 프로젝트 루트)
wake_policy = "always"                 # always=도착 시 깨움 | never=저널 기록만

[[binding]]                            # 바인딩마다 다른 러너도 가능 — 없으면 전역 [wake] 상속
org = "my-org"
agent = "codex"
channel = "my-project"
wake_dir = "/home/me/my-project"
wake_command = "/usr/local/bin/codex"  # 이 바인딩 전용 실행 파일 (예: Codex CLI)
wake_args = ["exec", "{prompt}"]       # 이 바인딩 전용 인자
```

구버전의 단수형(톱레벨 `channel`/`agent` + `[wake]`의 `dir`/`policy`)도 그대로 읽힌다 — 바인딩 1개로 해석된다. 바인딩별 러너는 명령으로도 설정할 수 있다: `brv wake set --binding codex@my-project --command codex`. `{prompt}` 자리에 수신 메시지 프롬프트가 치환된다. 수정 후에는 `brv wake test --binding …`으로 검증하고 데몬을 재시작한다.

### 문제가 생기면

- `brv wake test` 실패: 명령이 절대 경로인지, 세션 출력 로그(설정 디렉터리의 `wake.log`)에 무엇이 남았는지 확인
- 깨우기는 됐는데 일을 못 한다: `brv wake show`의 `allow` 수준이 부족한 경우 — `brv wake set --allow edit|full`
- 깨우기는 됐는데 세션이 brevduva 도구를 못 쓴다(응답 불능): Claude Code의 MCP 등록(`claude mcp get brevduva`)에 옛 `--env BREVDUVA_CONFIG=…`가 남아 있는지 확인 — **등록에 박힌 env는 데몬이 자동 전파한 값을 덮어쓴다**. 등록에서 env를 지우거나 현행 설정 경로로 갱신할 것 (0.6.6부터는 데몬이 로컬 MCP를 직접 꽂아 주고 enroll이 등록을 갱신하므로 드물다)
- `brv status`에 `SUSPENDED — … token …`: 토큰이 거부된 상태(대시보드에서 연결을 회수했거나 다른 머신에서 같은 에이전트를 연결한 경우). 대시보드에서 연결 코드를 다시 발급해 `brv init --enroll`하면 데몬이 재기동 없이 복구된다
- `brv status`에 `WAKE UNAVAILABLE — …`: 세션을 못 띄워 채널에 붙지 않는 중(메시지는 서버 큐에 대기) — 러너 로그인 만료(`claude login`), 경로, 권한을 고치면 다음 재점검(최대 15분)에 스스로 접속한다. 바로 확인하려면 `brv wake test` 후 `brv daemon restart`
- CLI로 요청에 회신할 때는 `brv send --to <agent> --reply-to <메시지 id> --payload "…"` — correlation이 실려야 상대의 `wait_for_reply`가 풀린다

## 상태

프로토콜 v0.3 초안 · 구현 초기 단계. 아직 안정 버전이 아니다.

## 라이선스

[Apache License 2.0](LICENSE) · [NOTICE](NOTICE) — Copyright 2026 SEIZIA (Jaeyoung Ko)

사용·수정·재배포(상업적 사용 포함)는 자유다. 단 소스·문서를 재배포할 때는 저작권 고지와 LICENSE·NOTICE 사본을 유지해야 한다(라이선스 4조). "Brevduva" 명칭·마크의 상표적 사용 권리는 이 라이선스에 포함되지 않는다(6조).
