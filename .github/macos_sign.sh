#!/bin/bash
# Copyright 2026 SEIZIA (Jaeyoung Ko)
# SPDX-License-Identifier: Apache-2.0
#
# macOS 실행 파일을 Developer ID로 서명하고 Apple 공증을 받는다 (2026-09-21).
# Gatekeeper의 "확인되지 않은 개발자" 차단을 없앤다. release.yml과 macos-sign-check.yml이 같이 쓴다.
#
#   .github/macos_sign.sh <실행 파일 경로>
#
# 환경변수(전부 GitHub Secrets에서): P12_BASE64 P12_PASSWORD API_KEY_P8_BASE64 API_KEY_ID API_ISSUER_ID
#
# - 맨 실행 파일에는 공증 티켓을 붙일(staple) 수 없다 — Gatekeeper가 첫 실행 때 온라인으로 조회한다
# - 서명된 빌드는 TeamIdentifier를 가지므로 brv의 토큰 주 저장소가 키체인으로 바뀐다
#   (crates/brv/src/config.rs keychain_is_reliable)
set -euo pipefail

bin="$1"
kc="$RUNNER_TEMP/sign.keychain-db"
kc_pw="$(uuidgen)"
cleanup() {
  rm -f "$RUNNER_TEMP/cert.p12" "$RUNNER_TEMP/AuthKey.p8"
  security delete-keychain "$kc" 2>/dev/null || true
}
trap cleanup EXIT

# Secret은 base64 문자만 걸러서 푼다 — 윈도우 PowerShell에서 `… | gh secret set`으로 넣으면 끝에 CR(LF),
# 환경에 따라 앞에 BOM이 붙고 macOS base64는 그런 바이트에서 거부한다 (2026-09-21 첫 시험 실행에서 실측)
decode_secret() { printf '%s' "$1" | LC_ALL=C tr -cd 'A-Za-z0-9+/=' | base64 --decode; }
# 같은 이유로 글자 값(암호·Key ID·Issuer ID)은 끝의 CR·LF만 뗀다
strip_eol() { local v="$1"; v="${v%$'\n'}"; v="${v%$'\r'}"; printf '%s' "$v"; }
P12_PASSWORD="$(strip_eol "$P12_PASSWORD")"
API_KEY_ID="$(strip_eol "$API_KEY_ID")"
API_ISSUER_ID="$(strip_eol "$API_ISSUER_ID")"

decode_secret "$P12_BASE64" > "$RUNNER_TEMP/cert.p12"
security create-keychain -p "$kc_pw" "$kc"
security set-keychain-settings -lut 21600 "$kc"
security unlock-keychain -p "$kc_pw" "$kc"
security import "$RUNNER_TEMP/cert.p12" -P "$P12_PASSWORD" -A -t cert -f pkcs12 -k "$kc"
security set-key-partition-list -S apple-tool:,apple: -k "$kc_pw" "$kc" >/dev/null
# shellcheck disable=SC2046
security list-keychains -d user -s "$kc" $(security list-keychains -d user | tr -d '"')
identity="$(security find-identity -v -p codesigning "$kc" | awk '/Developer ID Application/ {print $2; exit}')"
test -n "$identity" || { echo "::error::Developer ID Application identity not found in the imported .p12"; exit 1; }

codesign --force --sign "$identity" --options runtime --timestamp --identifier dev.brevduva.brv "$bin"
codesign --verify --strict --verbose=2 "$bin"
codesign -dv "$bin" 2>&1 | grep -E "^(Identifier|TeamIdentifier|Authority|Timestamp|flags)" || true

decode_secret "$API_KEY_P8_BASE64" > "$RUNNER_TEMP/AuthKey.p8"
ditto -c -k "$bin" "$RUNNER_TEMP/brv-notarize.zip"
# notarytool은 거절(Invalid)에도 0으로 끝날 수 있다 — 상태를 직접 확인하고, 거절이면 사유 로그를 남긴다
xcrun notarytool submit "$RUNNER_TEMP/brv-notarize.zip" \
  --key "$RUNNER_TEMP/AuthKey.p8" --key-id "$API_KEY_ID" --issuer "$API_ISSUER_ID" \
  --wait --output-format json | tee "$RUNNER_TEMP/notary.json"
if [ "$(jq -r .status "$RUNNER_TEMP/notary.json")" != "Accepted" ]; then
  xcrun notarytool log "$(jq -r .id "$RUNNER_TEMP/notary.json")" \
    --key "$RUNNER_TEMP/AuthKey.p8" --key-id "$API_KEY_ID" --issuer "$API_ISSUER_ID" || true
  echo "::error::notarization was not accepted"
  exit 1
fi
