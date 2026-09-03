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

# 인자 (2026-09-04, 온보딩 재설계 3): 대시보드가 만든 한 줄이 설치와 연결을 함께 한다 —
#   curl … | sh -s -- --server URL --enroll CODE [--unattended|--attended-only] [--runner ID]
# 같은 값을 env(BRV_SERVER·BRV_ENROLL)로도 받는다. 연결 뒤 무인 수신 질문은 brv init이 한다.
server="${BRV_SERVER:-https://api.brevduva.dev}"
enroll="${BRV_ENROLL:-}"
init_extra=""
while [ $# -gt 0 ]; do
  case "$1" in
    --server) server="$2"; shift 2 ;;
    --enroll) enroll="$2"; shift 2 ;;
    --runner) init_extra="$init_extra --runner $2"; shift 2 ;;
    --unattended | --attended-only) init_extra="$init_extra $1"; shift ;;
    *) echo "unknown option: $1 (accepted: --server URL --enroll CODE --runner ID --unattended --attended-only)" >&2; exit 2 ;;
  esac
done
REPO="froiibn/brevduva"
BASE="https://github.com/$REPO/releases/latest/download"

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Darwin)
    case "$arch" in
      arm64) target="aarch64-apple-darwin" ;;
      x86_64) target="x86_64-apple-darwin" ;;
      *) echo "unsupported macOS architecture: $arch" >&2; exit 1 ;;
    esac ;;
  Linux)
    case "$arch" in
      x86_64) target="x86_64-unknown-linux-musl" ;;
      aarch64 | arm64) target="aarch64-unknown-linux-musl" ;;
      *) echo "unsupported Linux architecture: $arch" >&2; exit 1 ;;
    esac ;;
  *) echo "unsupported OS: $os (use install.ps1 on Windows)" >&2; exit 1 ;;
esac

asset="brv-$target.tar.gz"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "downloading: $BASE/$asset"
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
echo "checksum verified"

tar -xzf "$tmp/$asset" -C "$tmp"
dest="${BRV_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$dest"
install -m 755 "$tmp/brv" "$dest/brv"

echo "installed: $dest/brv — $("$dest/brv" --version)"

# PATH 자동 등록 (2026-09-02, 실측 UX: 새 서버 첫 설치마다 수동 추가를 요구하던 것을 흡수).
# rustup/uv 관행을 따른다: 마커 달린 한 줄을 셸 설정에 추가, 이미 있으면 건너뜀(멱등).
# 대상: ~/.profile(로그인 셸, 없으면 생성) + ~/.bashrc(있을 때) + ~/.zshrc(있거나 zsh 사용 시).
case ":$PATH:" in
  *":$dest:"*) ;;
  *)
    if [ "${BRV_NO_MODIFY_PATH:-0}" = 1 ]; then
      echo "note: $dest is not on your PATH. Add it to your shell config:"
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
        echo "added $dest to PATH (${added# }) — new shells pick it up. For this shell right now:"
      else
        echo "the PATH entry is already in your shell config — new shells pick it up. For this shell right now:"
      fi
      echo "  export PATH=\"$dest:\$PATH\""
      case "${SHELL:-}" in
        *fish*) echo "for fish: fish_add_path $dest" ;;
      esac
    fi ;;
esac

# 갱신이면 돌고 있는 서비스 데몬을 새 바이너리로 재기동 (2026-09-03, 사용자 지적 "갱신마다
# launchctl을 쳐야 하나") — 서비스가 없으면 조용히 지나간다 (brv daemon restart의 미등록 오류 숨김)
"$dest/brv" daemon restart 2>/dev/null || true
if [ -n "$enroll" ]; then
  echo ""
  echo "connecting this machine (brv init) …"
  # `curl … | sh`에서는 stdin이 스크립트 파이프다 — brv init의 질문(무인 수신 여부·러너 선택)이
  # 터미널을 읽을 수 있게 /dev/tty를 준다. 터미널이 없으면 묻지 않고 플래그만 따른다
  if [ -r /dev/tty ]; then
    # shellcheck disable=SC2086
    "$dest/brv" init --server "$server" --enroll "$enroll" $init_extra < /dev/tty
  else
    # shellcheck disable=SC2086
    "$dest/brv" init --server "$server" --enroll "$enroll" $init_extra
  fi
else
  echo ""
  echo "next — connect this machine to your account:"
  echo "  1) https://brevduva.dev dashboard → Connect agents → Connect → copy the one-line command"
  echo "  2) paste it here (it runs: brv init --server $server --enroll <code>)"
fi
