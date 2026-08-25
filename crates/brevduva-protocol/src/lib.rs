//! Brevduva 프로토콜 공유 crate.
//!
//! [PROTOCOL.md](https://github.com/froiibn/brevduva/blob/main/PROTOCOL.md)의 코드화 —
//! 스펙이 진실이며 이 crate는 스펙을 따른다. 리시버(brv)와 서버가 함께 의존하므로
//! 여기 정의된 타입·검증이 프로토콜의 유일한 구현이다 (이중 구현 금지).
//!
//! 페이즈 1(IMPLEMENTATION.md)에서 채워질 모듈: 식별자, 주소, 엔벨로프, 컨트롤 프레임,
//! 능력 선언, 에러 코드, 토픽 와일드카드 매칭, JSON Schema 생성.

#![forbid(unsafe_code)]

/// 프로토콜 버전 — 엔벨로프의 `v` 필드 (PROTOCOL.md 3장).
pub const PROTOCOL_VERSION: u8 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    /// 회귀 테스트 씨앗 — 워크스페이스·CI 배선 검증용 스모크 테스트.
    #[test]
    fn protocol_version_is_v1() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
