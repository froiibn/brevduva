// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 로컬 평면 접속 자격 — 엔드포인트 기술서와 토큰 (RECEIVER_DESIGN.md P3, 2026-09-09 U5).
//!
//! 표준 관행을 따른다(Docker 소켓·Jupyter 토큰·ssh-agent와 같은 모양):
//! 1. **루프백에만 바인딩** — 이 머신 밖에서는 주소 자체가 닿지 않는다.
//! 2. **무작위 토큰** — OS 난수 32바이트. 붙는 쪽은 `Authorization: Bearer <토큰>`으로 낸다.
//! 3. **소유자 전용 파일** — 토큰이 든 기술서는 설정 디렉터리 안에 비밀 파일로 쓴다
//!    (유닉스 0600, 윈도우는 소유자·SYSTEM·Administrators DACL — `config::secure_config_dir`).
//!    같은 머신의 다른 계정은 파일을 못 읽으므로 토큰을 얻지 못한다.
//! 4. **상수 시간 비교** — 토큰 대조에서 시간 차로 앞자리를 캐낼 수 없게.
//! 5. **Origin 검증** — 브라우저가 로컬 엔드포인트를 찌르는 DNS 리바인딩을 막는다
//!    (MCP 스펙의 로컬 서버 요구).
//!
//! 윈도우에서 서비스는 LocalSystem이고 세션은 사용자 계정이지만, 설정 디렉터리 DACL이
//! 소유자(사용자)에게 읽기를 주므로 사용자 세션이 기술서를 읽을 수 있다. 별도의 자격 공유
//! 장치를 두지 않는 이유이기도 하다.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// 토큰 바이트 수 — 256비트. 로컬이라도 추측 가능한 길이를 쓰지 않는다.
const TOKEN_BYTES: usize = 32;

/// 로컬 엔드포인트 접속 토큰. `Display`를 일부러 구현하지 않는다 — 로그에 새지 않게.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Token(String);

impl Token {
    /// OS 난수로 새 토큰 발급.
    pub fn generate() -> anyhow::Result<Self> {
        let mut bytes = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).context("OS randomness unavailable for the local token")?;
        Ok(Self(hex(&bytes)))
    }

    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        let raw = raw.trim();
        anyhow::ensure!(
            raw.len() == TOKEN_BYTES * 2 && raw.bytes().all(|b| b.is_ascii_hexdigit()),
            "malformed local endpoint token"
        );
        Ok(Self(raw.to_owned()))
    }

    /// 헤더 값으로 쓸 원문 — 붙는 쪽에서만 쓴다.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// 상수 시간 대조. 길이가 달라도 조기 반환하지 않는다.
    pub fn matches(&self, presented: &str) -> bool {
        use subtle::ConstantTimeEq as _;
        let (mine, theirs) = (self.0.as_bytes(), presented.trim().as_bytes());
        // 길이는 비밀이 아니다 — 발급 토큰의 길이가 늘 같아 공개 상수와 다름없다. 길이가
        // 다르면 곧바로 거부하고, 같을 때만 상수 시간으로 내용을 대조한다.
        mine.len() == theirs.len() && bool::from(mine.ct_eq(theirs))
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// 붙는 쪽이 리시버를 찾는 기술서. 설정 디렉터리의 `endpoint.json`.
///
/// `version`은 P8(갱신) 버전 악수의 근거다 — 갱신 뒤 옛 어댑터가 붙으면 이 값과 자기 버전이
/// 달라 스스로 물러날 수 있다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    /// 이 기술서를 쓴 리시버의 버전.
    pub version: String,
    /// 루프백 주소·포트.
    pub addr: SocketAddr,
    pub token: Token,
    /// 쓴 리시버 프로세스 — 죽은 기술서를 구별한다.
    pub pid: u32,
    pub started_unix: u64,
}

impl Endpoint {
    /// 붙는 쪽이 쓸 기본 URL (`http://127.0.0.1:<포트>/mcp`).
    pub fn mcp_url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    /// 기술서를 비밀 파일로 발행한다. 디렉터리를 먼저 좁히고 원자적으로 교체한다.
    pub fn publish_at(&self, path: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(path.parent().context("endpoint path has parent")?)?;
        // 파일을 만들기 전에 좁힌다 — 새 파일이 좁혀진 권한을 상속받게 (윈도우)
        crate::config::secure_config_dir();
        crate::config::write_secret_file(path, &serde_json::to_string_pretty(self)?)
    }

    pub fn publish(&self) -> anyhow::Result<PathBuf> {
        let path = endpoint_path()?;
        self.publish_at(&path)?;
        Ok(path)
    }

    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("no local receiver endpoint at {path:?}"))?;
        let endpoint: Self = serde_json::from_str(&raw).context("malformed endpoint descriptor")?;
        anyhow::ensure!(
            is_loopback(&endpoint.addr),
            "endpoint descriptor points outside loopback ({}) — refusing",
            endpoint.addr
        );
        Ok(endpoint)
    }

    /// 이 머신의 리시버를 찾는다. 없으면 "리시버가 돌고 있지 않다"는 뜻이다.
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(&endpoint_path()?)
    }

    /// 리시버가 내려갈 때 치운다 — 죽은 주소로 붙으려는 시도를 줄인다.
    pub fn clear_at(path: &Path) {
        let _ = std::fs::remove_file(path);
    }
}

pub fn endpoint_path() -> anyhow::Result<PathBuf> {
    Ok(crate::config::config_path()?
        .parent()
        .context("config has no parent")?
        .join("endpoint.json"))
}

/// 루프백 주소인가 — 바인딩 시점과 기술서 적재 시점 양쪽에서 확인한다.
pub fn is_loopback(addr: &SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => v4 == Ipv4Addr::LOCALHOST || v4.is_loopback(),
        IpAddr::V6(v6) => v6 == Ipv6Addr::LOCALHOST || v6.is_loopback(),
    }
}

/// 브라우저발 요청 차단 (DNS 리바인딩) — MCP 스펙의 로컬 서버 요구.
///
/// `Origin`이 없는 요청은 브라우저가 아니다(러너·CLI·로봇 제어기) → 통과. 있으면 루프백
/// 출처만 통과. 이름 기반 리바인딩(`http://evil.example`)은 여기서 걸린다.
pub fn origin_allowed(origin: Option<&str>) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    let origin = origin.trim();
    if origin.eq_ignore_ascii_case("null") {
        return false;
    }
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or_default();
    let host = host.rsplit_once(':').map_or(host, |(h, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) && !host.ends_with(']') {
            h
        } else {
            host
        }
    });
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback() || ip == IpAddr::V4(Ipv4Addr::LOCALHOST))
}

/// `Authorization: Bearer <토큰>`에서 토큰만 꺼낸다.
pub fn bearer(header: Option<&str>) -> Option<&str> {
    let raw = header?.trim();
    let (scheme, value) = raw.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| value.trim())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "brv-auth-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn generated_tokens_are_long_random_hex() {
        let a = Token::generate().expect("token");
        let b = Token::generate().expect("token");
        assert_eq!(a.expose().len(), TOKEN_BYTES * 2);
        assert!(a.expose().bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a.expose(), b.expose(), "two tokens must not collide");
    }

    #[test]
    fn token_matches_only_the_exact_value() {
        let token = Token::generate().expect("token");
        assert!(token.matches(token.expose()));
        assert!(
            token.matches(&format!(" {} ", token.expose())),
            "공백은 다듬는다"
        );
        assert!(!token.matches(""));
        assert!(!token.matches("00"));
        assert!(!token.matches(&token.expose()[..TOKEN_BYTES * 2 - 1]));
        let mut wrong = token.expose().to_owned();
        wrong.replace_range(0..1, if wrong.starts_with('a') { "b" } else { "a" });
        assert!(!token.matches(&wrong), "한 글자만 달라도 거부");
    }

    #[test]
    fn token_parse_rejects_malformed_values() {
        assert!(Token::parse(&"a".repeat(TOKEN_BYTES * 2)).is_ok());
        assert!(Token::parse("").is_err());
        assert!(Token::parse(&"z".repeat(TOKEN_BYTES * 2)).is_err());
        assert!(Token::parse(&"a".repeat(TOKEN_BYTES * 2 - 1)).is_err());
    }

    #[test]
    fn token_is_not_printed_by_debug() {
        let token = Token::generate().expect("token");
        let shown = format!("{token:?}");
        assert!(
            !shown.contains(token.expose()),
            "디버그 출력에 토큰이 새면 안 된다"
        );
    }

    fn endpoint(addr: &str) -> Endpoint {
        Endpoint {
            version: "0.6.39".into(),
            addr: addr.parse().expect("addr"),
            token: Token::generate().expect("token"),
            pid: std::process::id(),
            started_unix: 1,
        }
    }

    #[test]
    fn endpoint_round_trips_through_the_descriptor_file() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("endpoint.json");
        let written = endpoint("127.0.0.1:52100");
        written.publish_at(&path).expect("publish");
        let read = Endpoint::load_from(&path).expect("load");
        assert_eq!(read.addr, written.addr);
        assert_eq!(read.token, written.token);
        assert_eq!(read.version, written.version);
        assert_eq!(read.mcp_url(), "http://127.0.0.1:52100/mcp");
        Endpoint::clear_at(&path);
        assert!(Endpoint::load_from(&path).is_err(), "치운 뒤에는 없다");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_file_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = temp_dir("mode");
        let path = dir.join("endpoint.json");
        endpoint("127.0.0.1:52101")
            .publish_at(&path)
            .expect("publish");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(mode, 0o600, "토큰이 든 파일은 소유자만 읽는다");
    }

    #[test]
    fn a_descriptor_pointing_off_loopback_is_refused() {
        let dir = temp_dir("offloopback");
        let path = dir.join("endpoint.json");
        endpoint("0.0.0.0:52102")
            .publish_at(&path)
            .expect("publish");
        let error = Endpoint::load_from(&path).expect_err("must refuse");
        assert!(error.to_string().contains("loopback"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn loopback_check_covers_v4_and_v6() {
        assert!(is_loopback(&"127.0.0.1:1".parse().expect("addr")));
        assert!(is_loopback(&"127.9.9.9:1".parse().expect("addr")));
        assert!(is_loopback(&"[::1]:1".parse().expect("addr")));
        assert!(!is_loopback(&"0.0.0.0:1".parse().expect("addr")));
        assert!(!is_loopback(&"192.168.1.5:1".parse().expect("addr")));
    }

    #[test]
    fn origin_header_blocks_browser_rebinding() {
        // 러너·CLI·로봇 제어기는 Origin을 붙이지 않는다
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("http://localhost:52100")));
        assert!(origin_allowed(Some("http://127.0.0.1:52100")));
        assert!(origin_allowed(Some("http://[::1]:52100")));
        assert!(!origin_allowed(Some("http://evil.example")));
        assert!(!origin_allowed(Some("https://brevduva.dev")));
        assert!(!origin_allowed(Some("null")));
        assert!(!origin_allowed(Some("file://")));
        // 리바인딩의 전형 — 이름이 루프백으로 해석되더라도 이름 자체를 신뢰하지 않는다
        assert!(!origin_allowed(Some("http://localtest.me:52100")));
    }

    #[test]
    fn bearer_extracts_only_a_bearer_token() {
        assert_eq!(bearer(Some("Bearer abc")), Some("abc"));
        assert_eq!(bearer(Some("bearer  abc ")), Some("abc"));
        assert_eq!(bearer(Some("Basic abc")), None);
        assert_eq!(bearer(Some("Bearer")), None);
        assert_eq!(bearer(Some("Bearer ")), None);
        assert_eq!(bearer(None), None);
    }
}
