//! brv — Brevduva 리시버·CLI의 라이브러리 표면.
//!
//! 바이너리(main)와 서버 리포의 E2E 통합 테스트가 같은 코드를 쓴다.
//! `client`는 PROTOCOL.md 13장(장애·재연결)의 참조 구현이다.

#![forbid(unsafe_code)]

pub mod client;
pub mod config;
pub mod mcp;
