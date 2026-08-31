// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 스키마 스냅샷 테스트 — `schemas/`가 타입 정의와 1:1인지 강제한다.
//!
//! 스키마가 달라지는 변경은 `BREVDUVA_UPDATE_SCHEMAS=1 cargo test`로 스냅샷을
//! 재생성해 함께 커밋한다 — 우발적 와이어 포맷 변경을 리뷰 가능한 diff로 만드는 장치.

use std::fs;
use std::path::PathBuf;

fn schemas_dir() -> PathBuf {
    // crate → 리포 루트의 schemas/ (리포 구조는 IMPLEMENTATION.md)
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas")
}

#[test]
fn schemas_match_committed_snapshots() {
    let dir = schemas_dir();
    let update = std::env::var_os("BREVDUVA_UPDATE_SCHEMAS").is_some();
    if update {
        fs::create_dir_all(&dir).unwrap();
    }

    for (name, schema) in brevduva_protocol::root_schemas() {
        let path = dir.join(format!("{name}.json"));
        let rendered = serde_json::to_string_pretty(&schema).unwrap() + "\n";
        if update {
            fs::write(&path, &rendered).unwrap();
            continue;
        }
        let committed = fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "missing snapshot {path:?} ({e}); run `BREVDUVA_UPDATE_SCHEMAS=1 cargo test` and commit schemas/"
            )
        });
        // 개행 차이(CRLF)에 흔들리지 않게 JSON 값으로 비교한다
        let committed_value: serde_json::Value = serde_json::from_str(&committed).unwrap();
        assert_eq!(
            schema, committed_value,
            "schema {name:?} drifted from schemas/{name}.json — if intentional, regenerate with BREVDUVA_UPDATE_SCHEMAS=1 and commit"
        );
    }
}
