// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 로컬 평면 — 리시버와 이 머신의 세션들 사이의 평면 (RECEIVER_DESIGN.md P2·P3).
//!
//! 서버에 붙는 것은 리시버 하나이고(P2), 세션·CLI·GUI 어댑터는 전부 여기로 붙는다(P3).
//! 종전에는 세션마다 자기 토큰으로 서버에 JOIN해 리시버와 자리를 다퉜다 — 그 구조에서
//! 비롯된 것이 데몬 standby 왕복, 설치기 재기동 시 라이브 세션 밀어내기, 갱신 뒤 옛 어댑터가
//! 옛 코드로 서버와 대화하던 문제였다.
//!
//! 층 구분 (로봇 로드맵 대비 — RECEIVER_REBUILD_PLAN §1):
//! - [`registry`] — 정체성·잠금·러너 입력 통로. `Envelope`와 능력 선언만 안다. MCP도 HTTP도 모른다.
//! - 프런트엔드 — [`http`](MCP Streamable HTTP)와 [`bridge`](stdio 중계). 유닉스 소켓·명명 파이프·
//!   로봇 제어기가 같은 자리에 꽂힌다.
//! - [`plane`] — 도구·라우팅(P5·P6)·전달 기록. 러너 입력 통로(Monitor 등)의 수명을 소유한다.

pub mod auth;
pub mod bridge;
pub mod http;
pub mod plane;
pub mod registry;
pub mod runner_exec;

pub use auth::{Endpoint, Token};
pub use http::{LocalHttp, SessionHandler};
pub use plane::{BindingConnection, Plane, Routed};
pub use registry::{
    AttachSpec, Became, BindingKey, DeliveryTarget, Held, Hold, HoldOwner, Origin, PushError,
    PushEvent, Registry, Session, SessionCapabilities, SessionId, Sink, TargetKind, WakeId,
};
pub use runner_exec::{ExecOutput, RunnerExec, UserContextExec};
