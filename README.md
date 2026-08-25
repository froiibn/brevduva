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

## 상태

프로토콜 v0.3 초안 · 구현 초기 단계. 아직 안정 버전이 아니다.

## 라이선스

[Apache 2.0](LICENSE)
