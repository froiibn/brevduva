#!/bin/bash
# Copyright 2026 SEIZIA (Jaeyoung Ko)
# SPDX-License-Identifier: Apache-2.0
#
# brv 실행 파일을 Brevduva.app 묶음으로 조립한다 (2026-09-21). 서명은 macos_sign.sh가 한다.
#
#   .github/macos_bundle.sh <brv 실행 파일> <출력 디렉터리> <타깃 트리플>  →  <출력 디렉터리>/Brevduva.app
#
# 묶음 구성:
#   Contents/MacOS/brv                                     리시버 본체
#   Contents/MacOS/brevduva-service                        SMAppService 등록 도구 (packaging/macos/service.swift)
#   Contents/Library/LaunchAgents/dev.brevduva.brv-daemon.plist   그 도구가 등록하는 서비스 정의
#   Contents/Resources/{Brevduva.icns, LICENSE}
#
# 리포 루트에서 실행한다 (packaging/macos/, Cargo.toml, LICENSE를 읽는다).
set -euo pipefail

bin="$1"
out="$2"
triple="$3"
app="$out/Brevduva.app"
# Cargo.toml은 UTF-8 BOM으로 시작한다 — 줄 앞을 고정하지 않고 [workspace.package]의 version을 집는다
version="$(grep -m1 -E '^version *= *"' Cargo.toml | cut -d'"' -f2)"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]] || { echo "::error::could not read the version from Cargo.toml"; exit 1; }
case "$triple" in
  aarch64-apple-darwin) swift_target="arm64-apple-macos13.0" ;;
  x86_64-apple-darwin) swift_target="x86_64-apple-macos13.0" ;;
  *) echo "::error::unsupported target $triple"; exit 1 ;;
esac

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Library/LaunchAgents"
install -m 755 "$bin" "$app/Contents/MacOS/brv"
# SMAppService는 macOS 13부터다 — 도구만 13을 요구하고 brv 본체의 최소 버전은 그대로 둔다
swiftc -O -target "$swift_target" packaging/macos/service.swift -o "$app/Contents/MacOS/brevduva-service"
sed "s/__VERSION__/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
cp packaging/macos/dev.brevduva.brv-daemon.plist "$app/Contents/Library/LaunchAgents/"
cp packaging/macos/Brevduva.icns "$app/Contents/Resources/Brevduva.icns"
cp LICENSE "$app/Contents/Resources/LICENSE"
plutil -lint "$app/Contents/Info.plist" "$app/Contents/Library/LaunchAgents/dev.brevduva.brv-daemon.plist"
echo "bundled: $app (version $version, $triple)"
