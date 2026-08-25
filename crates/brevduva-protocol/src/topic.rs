//! 토픽 path와 구독 필터 (PROTOCOL.md 2.3).
//!
//! - path: `[a-z0-9-]` 세그먼트를 `.`으로 연결 (예: `api-changes.auth`)
//! - 필터: 발행 path 문법 + 와일드카드 — `*`(정확히 한 세그먼트), `>`(이하 전부, 마지막만)
//! - 매칭 시맨틱은 tibrv/NATS 계열 관례에서 차용 (문법만 차용, 구현 의존 없음)

use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use crate::error::ParseError;

/// 세그먼트 하나의 최대 길이. 식별자와 동일 상한을 공유한다.
const SEGMENT_MAX_LEN: usize = 64;

fn parse_segments(input: &str, allow_wildcards: bool) -> Result<Vec<String>, ParseError> {
    if input.is_empty() {
        return Err(ParseError::InvalidTopic {
            input: input.to_owned(),
            reason: "empty",
        });
    }
    let segments: Vec<&str> = input.split('.').collect();
    for (i, seg) in segments.iter().enumerate() {
        let is_last = i == segments.len() - 1;
        match *seg {
            "" => {
                return Err(ParseError::InvalidTopic {
                    input: input.to_owned(),
                    reason: "empty segment (leading/trailing/double dot)",
                });
            }
            "*" if allow_wildcards => {}
            ">" if allow_wildcards => {
                if !is_last {
                    return Err(ParseError::InvalidTopic {
                        input: input.to_owned(),
                        reason: "`>` must be the last segment",
                    });
                }
            }
            s => {
                if s.len() > SEGMENT_MAX_LEN {
                    return Err(ParseError::InvalidTopic {
                        input: input.to_owned(),
                        reason: "segment longer than 64 characters",
                    });
                }
                if !s
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                {
                    return Err(ParseError::InvalidTopic {
                        input: input.to_owned(),
                        reason: "segment contains characters outside [a-z0-9-]",
                    });
                }
            }
        }
    }
    Ok(segments.into_iter().map(str::to_owned).collect())
}

/// 발행용 토픽 path — 와일드카드 없음.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TopicPath(Vec<String>);

impl TopicPath {
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        Ok(Self(parse_segments(input, false)?))
    }

    pub fn segments(&self) -> &[String] {
        &self.0
    }
}

/// 구독용 토픽 필터 — `*`/`>` 와일드카드 허용.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TopicFilter(Vec<String>);

impl TopicFilter {
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        Ok(Self(parse_segments(input, true)?))
    }

    /// 이 필터가 발행 path에 매칭되는가.
    ///
    /// 규칙(tibrv/NATS 관례): `*`는 정확히 한 세그먼트, `>`는 남은 1개 이상 전부.
    /// 따라서 `a.>`는 `a`에 매칭되지 않는다 (`>`가 최소 1세그먼트를 요구).
    pub fn matches(&self, path: &TopicPath) -> bool {
        let mut path_iter = path.segments().iter();
        for filter_seg in &self.0 {
            match filter_seg.as_str() {
                ">" => return path_iter.next().is_some(),
                "*" => {
                    if path_iter.next().is_none() {
                        return false;
                    }
                }
                literal => {
                    if path_iter.next().map(String::as_str) != Some(literal) {
                        return false;
                    }
                }
            }
        }
        path_iter.next().is_none()
    }
}

macro_rules! topic_string_impls {
    ($ty:ident, $desc:literal, $pattern:literal) => {
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0.join("."))
            }
        }
        impl TryFrom<String> for $ty {
            type Error = ParseError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(&value)
            }
        }
        impl From<$ty> for String {
            fn from(value: $ty) -> Self {
                value.to_string()
            }
        }
        impl JsonSchema for $ty {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($ty).into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "string",
                    "description": $desc,
                    "pattern": $pattern
                })
            }
        }
    };
}

topic_string_impls!(
    TopicPath,
    "publish topic path: dot-joined [a-z0-9-] segments (PROTOCOL.md 2.3)",
    "^[a-z0-9-]+(\\.[a-z0-9-]+)*$"
);
topic_string_impls!(
    TopicFilter,
    "subscription topic filter: publish path syntax plus wildcards `*` (one segment) and `>` (rest, last position only) (PROTOCOL.md 2.3)",
    "^(\\*|>|[a-z0-9-]+)(\\.(\\*|[a-z0-9-]+))*(\\.>)?$"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_paths_and_rejects_wildcards_in_paths() {
        assert!(TopicPath::parse("api-changes.auth").is_ok());
        for bad in ["", ".", "a..b", "a.", "A.b", "a.*", ">", "a.>"] {
            assert!(TopicPath::parse(bad).is_err(), "should reject path {bad:?}");
        }
    }

    #[test]
    fn parses_filters() {
        for ok in ["a", "a.*", "a.>", ">", "*.b", "a.*.c"] {
            assert!(
                TopicFilter::parse(ok).is_ok(),
                "should accept filter {ok:?}"
            );
        }
        for bad in [">.a", "a.>.b", "a.**", ""] {
            assert!(
                TopicFilter::parse(bad).is_err(),
                "should reject filter {bad:?}"
            );
        }
    }

    /// 와일드카드 매칭 표 — 회귀 테스트셋의 핵심. 시맨틱 변경은 이 표의 의도적 수정으로만.
    #[test]
    fn wildcard_matching_table() {
        let cases: &[(&str, &str, bool)] = &[
            ("a.b", "a.b", true),
            ("a.b", "a.c", false),
            ("a.*", "a.b", true),
            ("a.*", "a", false),
            ("a.*", "a.b.c", false),
            ("a.>", "a.b", true),
            ("a.>", "a.b.c", true),
            ("a.>", "a", false), // `>`는 최소 1세그먼트
            (">", "a", true),
            (">", "a.b.c", true),
            ("*.b", "a.b", true),
            ("*.b", "a.c", false),
            ("api-changes.*", "api-changes.auth", true),
        ];
        for (filter, path, expected) in cases {
            let f = TopicFilter::parse(filter).unwrap();
            let p = TopicPath::parse(path).unwrap();
            assert_eq!(
                f.matches(&p),
                *expected,
                "filter {filter:?} vs path {path:?}"
            );
        }
    }

    #[test]
    fn serde_round_trip() {
        let p: TopicPath = serde_json::from_str("\"api-changes.auth\"").unwrap();
        assert_eq!(serde_json::to_string(&p).unwrap(), "\"api-changes.auth\"");
        let f: TopicFilter = serde_json::from_str("\"api-changes.>\"").unwrap();
        assert_eq!(serde_json::to_string(&f).unwrap(), "\"api-changes.>\"");
    }
}
