//! brv — Brevduva 리시버·CLI의 라이브러리 표면.
//!
//! 바이너리(main)와 서버 리포의 E2E 통합 테스트가 같은 코드를 쓴다.
//! `client`는 PROTOCOL.md 13장(장애·재연결)의 참조 구현이다.

// forbid → deny (2026-08-27, 페이즈 7): 윈도우 서비스 로그온 권한(LSA) 부여만 FFI가
// 불가피하다 — service.rs의 해당 함수에 한해 근거 주석과 함께 allow. 그 외 전역 금지 유지
#![deny(unsafe_code)]

pub mod client;
pub mod config;
pub mod daemon;
pub mod mcp;
pub mod service;
