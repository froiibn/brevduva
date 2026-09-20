#!/bin/bash
# Copyright 2026 SEIZIA (Jaeyoung Ko)
# SPDX-License-Identifier: Apache-2.0
#
# brv 실행 파일을 Brevduva.app 묶음으로 조립한다 (2026-09-21). 서명은 macos_sign.sh가 한다.
#
#   .github/macos_bundle.sh <brv 실행 파일> <출력 디렉터리>   →  <출력 디렉터리>/Brevduva.app
#
# 리포 루트에서 실행한다 (packaging/macos/, Cargo.toml, LICENSE를 읽는다).
set -euo pipefail

bin="$1"
out="$2"
app="$out/Brevduva.app"
# Cargo.toml은 UTF-8 BOM으로 시작한다 — 줄 앞을 고정하지 않고 [workspace.package]의 version을 집는다
version="$(grep -m1 -E '^version *= *"' Cargo.toml | cut -d'"' -f2)"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]] || { echo "::error::could not read the version from Cargo.toml"; exit 1; }

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m 755 "$bin" "$app/Contents/MacOS/brv"
sed "s/__VERSION__/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
cp packaging/macos/Brevduva.icns "$app/Contents/Resources/Brevduva.icns"
cp LICENSE "$app/Contents/Resources/LICENSE"
plutil -lint "$app/Contents/Info.plist"
echo "bundled: $app (version $version)"
