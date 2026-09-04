// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! brv — Brevduva 리시버·CLI의 라이브러리 표면.
//!
//! 바이너리(main)와 서버 리포의 E2E 통합 테스트가 같은 코드를 쓴다.
//! `client`는 PROTOCOL.md 13장(장애·재연결)의 참조 구현이다.

// forbid → deny (2026-08-27, 페이즈 7): Win32 FFI가 불가피한 곳만 근거 주석과 함께 allow —
// 현재는 winspawn.rs(로그온한 사용자 세션에 깨우기 스폰, 2026-09-03) 하나. 그 외 전역 금지
#![deny(unsafe_code)]

pub mod client;
pub mod config;
pub mod daemon;
pub mod enroll;
pub mod hook;
pub mod manage;
pub mod mcp;
pub mod runners;
pub mod service;
#[cfg(windows)]
pub mod winspawn;
