//! brv — Brevduva 리시버 데몬 + CLI.
//!
//! 페이즈 5(IMPLEMENTATION.md)에서 구현: WS 클라이언트 코어(재연결 백오프·client_key
//! 멱등성), `brv init`/`brv status`, OS 서비스 데몬, 로컬 MCP 서버.
//! 지금은 워크스페이스 배선 검증용 최소 진입점.

fn main() {
    println!(
        "brv {} (protocol v{})",
        env!("CARGO_PKG_VERSION"),
        brevduva_protocol::PROTOCOL_VERSION
    );
}
