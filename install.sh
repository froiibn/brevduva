#!/bin/sh
# Copyright 2026 SEIZIA (Jaeyoung Ko) — SPDX-License-Identifier: Apache-2.0
# Brevduva 리시버(brv) 설치 — macOS(arm64/x86_64) · Linux(x86_64/aarch64, musl 정적 바이너리)
#
#   curl -fsSL https://brevduva.dev/install.sh | sh
#
# 하는 일: GitHub Releases 최신 버전에서 플랫폼에 맞는 바이너리를 받아
# SHA256 검증 후 ~/.local/bin/brv 로 설치하고(BRV_INSTALL_DIR로 변경 가능),
# 설치 경로가 PATH에 없으면 셸 설정에 마커 달린 한 줄을 추가한다
# (BRV_NO_MODIFY_PATH=1 로 거부 가능 — 그때는 안내만 출력). 그 외에는 건드리지 않는다.
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

# PATH 자동 등록 (2026-09-02, 실측 UX: 새 서버 첫 설치마다 수동 추가를 요구하던 것을 흡수).
# rustup/uv 관행을 따른다: 마커 달린 한 줄을 셸 설정에 추가, 이미 있으면 건너뜀(멱등).
# 대상: ~/.profile(로그인 셸, 없으면 생성) + ~/.bashrc(있을 때) + ~/.zshrc(있거나 zsh 사용 시).
case ":$PATH:" in
  *":$dest:"*) ;;
  *)
    if [ "${BRV_NO_MODIFY_PATH:-0}" = 1 ]; then
      echo "주의: $dest 가 PATH에 없습니다. 셸 설정에 추가하세요:"
      echo "  export PATH=\"$dest:\$PATH\""
    else
      # $HOME 하위면 \$HOME 변수로 기록 — 리터럴 경로보다 이식성 좋음
      case "$dest" in
        "$HOME"/*) path_line="export PATH=\"\$HOME/${dest#"$HOME"/}:\$PATH\" # brevduva installer" ;;
        *) path_line="export PATH=\"$dest:\$PATH\" # brevduva installer" ;;
      esac
      added=""
      for rc in "$HOME/.profile" "$HOME/.bashrc" "$HOME/.zshrc"; do
        case "$rc" in
          "$HOME/.bashrc") [ -e "$rc" ] || continue ;;
          "$HOME/.zshrc") if [ ! -e "$rc" ]; then case "${SHELL:-}" in *zsh*) ;; *) continue ;; esac; fi ;;
        esac
        grep -qs "# brevduva installer" "$rc" && continue
        printf '\n%s\n' "$path_line" >> "$rc"
        added="$added ${rc##*/}"
      done
      if [ -n "$added" ]; then
        echo "PATH에 $dest 추가됨($added ) — 새 셸부터 적용. 지금 이 셸에서 바로 쓰려면:"
      else
        echo "PATH 항목은 셸 설정에 이미 있습니다 — 새 셸부터 적용. 지금 이 셸에서 바로 쓰려면:"
      fi
      echo "  export PATH=\"$dest:\$PATH\""
      case "${SHELL:-}" in
        *fish*) echo "fish 사용 시: fish_add_path $dest" ;;
      esac
    fi ;;
esac

echo ""
echo "다음 단계 — 계정과 이 머신을 연결하세요:"
echo "  1) https://brevduva.dev 대시보드 → 머신 연결에서 등록 코드 발급 (brvenr_ 로 시작)"
echo "  2) brv init --server https://api.brevduva.dev --enroll <코드>"
