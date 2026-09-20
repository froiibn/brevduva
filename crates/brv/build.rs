// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 윈도우 실행 파일에 아이콘과 버전 정보를 심는다 (2026-09-21) — 없으면 탐색기·작업 관리자·
//! 서비스 속성에 기본 아이콘과 빈 "자세히" 탭으로 나온다. 버전·설명은 Cargo 메타데이터에서
//! 자동으로 채워진다. macOS·리눅스의 맨 실행 파일은 아이콘을 담는 형식이 없어 대상이 아니다.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/brv.ico");
    // cfg!(windows)가 아니라 타깃을 본다 — build.rs는 호스트에서 돌기 때문
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/brv.ico")
        .set("ProductName", "Brevduva")
        .set("FileDescription", "Brevduva receiver (brv)")
        .set("CompanyName", "SEIZIA")
        .set("LegalCopyright", "Copyright 2026 SEIZIA (Jaeyoung Ko)");
    // 리소스 컴파일러(GNU는 windres, MSVC는 rc.exe)가 없는 개발 환경에서 빌드를 막지 않는다 —
    // 아이콘 없는 exe가 나올 뿐이다. 릴리스에서 빠지는 것은 release.yml의 검사가 잡는다
    if let Err(e) = res.compile() {
        println!("cargo:warning=windows resources not embedded: {e}");
    }
}
