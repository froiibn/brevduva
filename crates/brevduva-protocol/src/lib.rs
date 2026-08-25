//! Brevduva 프로토콜 공유 crate.
//!
//! [PROTOCOL.md](https://github.com/froiibn/brevduva/blob/main/PROTOCOL.md)의 코드화 —
//! 스펙이 진실이며 이 crate는 스펙을 따른다. 리시버(brv)와 서버가 함께 의존하므로
//! 여기 정의된 타입·검증이 프로토콜의 유일한 구현이다 (이중 구현 금지).
//!
//! 모듈 ↔ 스펙 장 대응: ident/topic/address(2장) · envelope/ids(3장) · capability(4장)
//! · frame(5.2·7장) · errcode(8장) · schema(15장 정식화 산출물 `schemas/`).

#![forbid(unsafe_code)]

mod address;
mod capability;
mod envelope;
mod errcode;
mod error;
mod frame;
mod ident;
mod ids;
mod schema;
mod topic;

pub use address::Address;
pub use capability::{Capabilities, ReceiveMode};
pub use envelope::{Envelope, Expects, Kind, PayloadRef};
pub use errcode::ErrorCode;
pub use error::ParseError;
pub use frame::{
    ClientFrame, ClientOp, ErrBody, OkBody, PresenceEntry, PresenceState, ServerFrame, ServerOp,
};
pub use ident::{IDENT_MAX_LEN, Ident};
pub use ids::{ClientKey, MessageId, Timestamp};
pub use schema::root_schemas;
pub use topic::{TopicFilter, TopicPath};

/// 프로토콜 버전 — 엔벨로프의 `v` 필드 (PROTOCOL.md 3장).
pub const PROTOCOL_VERSION: u8 = 1;
