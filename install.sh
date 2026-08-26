#!/bin/sh
# Brevduva 리시버(brv) 설치 — macOS(arm64/x86_64) · Linux(x86_64/aarch64, musl 정적 바이너리)
#
#   curl -fsSL https://brevduva.dev/install.sh | sh
#
# 하는 일: GitHub Releases 최신 버전에서 플랫폼에 맞는 바이너리를 받아
# SHA256 검증 후 ~/.local/bin/brv 로 설치한다 (BRV_INSTALL_DIR로 변경 가능).
# 그 외에는 아무것도 건드리지 않는다.
set -eu

REPO="froiibn/brevduva"
BASE="https://github.com/$REPO/releases/latest/download"

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Darwin)
    case "$arch" in
      arm64) target="aarch64-apple-darwin" ;;
      x86_64) target="x86_64-apple-darwin" ;;
      *) echo "지원하지 않는 macOS 아키텍처: $arch" >&2; exit 1 ;;
    esac ;;
  Linux)
    case "$arch" in
      x86_64) target="x86_64-unknown-linux-musl" ;;
      aarch64 | arm64) target="aarch64-unknown-linux-musl" ;;
      *) echo "지원하지 않는 Linux 아키텍처: $arch" >&2; exit 1 ;;
    esac ;;
  *) echo "지원하지 않는 OS: $os (Windows는 install.ps1 사용)" >&2; exit 1 ;;
esac

asset="brv-$target.tar.gz"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "다운로드: $BASE/$asset"
curl -fsSL "$BASE/$asset" -o "$tmp/$asset"
curl -fsSL "$BASE/SHA256SUMS" -o "$tmp/SHA256SUMS"

# 체크섬 검증 — sha256sum(리눅스) / shasum(macOS)
grep " $asset\$" "$tmp/SHA256SUMS" > "$tmp/sum.txt"
(
  cd "$tmp"
  if command -v sha256sum > /dev/null 2>&1; then
    sha256sum -c sum.txt > /dev/null
  else
    shasum -a 256 -c sum.txt > /dev/null
  fi
)
echo "체크섬 확인 완료"

tar -xzf "$tmp/$asset" -C "$tmp"
dest="${BRV_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$dest"
install -m 755 "$tmp/brv" "$dest/brv"

echo "설치 완료: $dest/brv — $("$dest/brv" --version)"
case ":$PATH:" in
  *":$dest:"*) ;;
  *) echo "주의: $dest 가 PATH에 없습니다. 셸 설정에 추가하세요:"
     echo "  export PATH=\"$dest:\$PATH\"" ;;
esac
