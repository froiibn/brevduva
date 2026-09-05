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
#
# 진행 표시 (2026-09-05, 사용자 지적 "텍스트만 나와서 설치 중에 멈춘 것처럼 보인다"): 터미널이면
# 단계 진척률 막대 + 지금 하는 일을 한 줄로 갱신하고, 다운로드는 받은 양/전체와 초당 속도를 보인다.
# 터미널이 아니면(CI·로그 파이프) 단계마다 한 줄씩만 남긴다. 막대는 stderr, 결과 메시지는 stdout.
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

# ---- 진행 표시 ----
STEPS=7
step=0
if [ -t 2 ]; then tty=1; else tty=0; fi
# 막대 한 줄 갱신 (터미널만). $1 = 0..100, $2 = 지금 하는 일
draw() {
  [ "$tty" = 1 ] || return 0
  filled=$(($1 * 24 / 100)); i=0; b=""
  while [ "$i" -lt 24 ]; do
    if [ "$i" -lt "$filled" ]; then b="${b}#"; else b="${b}-"; fi
    i=$((i + 1))
  done
  printf '\r\033[K[%s] %3d%%  %s' "$b" "$1" "$2" >&2
}
# 다음 단계로 — 막대는 (끝난 단계 / 전체). 비터미널은 한 줄 로그
step() {
  step=$((step + 1))
  pct=$(((step - 1) * 100 / STEPS))
  if [ "$tty" = 1 ]; then draw "$pct" "$1"; else printf '[%3d%%] %s\n' "$pct" "$1" >&2; fi
}
# 영구 메시지 — 막대 줄을 비우고 stdout에 남긴다 (막대는 다음 step에서 다시 그려진다)
line() {
  [ "$tty" = 1 ] && printf '\r\033[K' >&2
  echo "$1"
}
# 바이트 → 사람용 크기 (1 MB 이상은 소수 한 자리 MB, 1 KB 이상은 KB, 그 아래는 B)
human() {
  if [ "$1" -ge 1048576 ]; then printf '%d.%d MB' $(($1 / 1048576)) $(($1 % 1048576 * 10 / 1048576))
  elif [ "$1" -ge 1024 ]; then printf '%d KB' $(($1 / 1024))
  else printf '%d B' "$1"; fi
}
# 다운로드 — curl을 뒤에서 돌리고 0.2초마다 파일 크기를 읽어 받은 양/전체·초당 속도를 그린다.
# 전체 크기는 응답 헤더(-D)에서 얻되 **200 응답의** Content-Length만 믿는다 — 리다이렉트(302)의
# `content-length: 0`이 먼저 도착하므로 그것을 전체로 잡으면 전체가 영영 안 보인다 (2026-09-05 실측).
# $1 = url, $2 = 저장 경로, $3 = 표시 이름, $4 = "quiet"면 완료 줄을 남기지 않는다
fetch() {
  hdr="$2.headers"
  : > "$2"
  curl -fsSL -S -D "$hdr" -o "$2" "$1" &
  pid=$!
  total=""; have=0; prev=0; prev_t=$(date +%s); start_t=$prev_t; speed=0
  base=$(((step - 1) * 100 / STEPS)); span=$((100 / STEPS))
  while kill -0 "$pid" 2>/dev/null; do
    sleep 0.2
    [ -n "$total" ] || total=$(tr -d '\r' < "$hdr" 2>/dev/null | awk 'toupper($1) ~ /^HTTP\// { code = $2; v = "" } tolower($1) == "content-length:" && code == "200" { v = $2 } END { print v }')
    have=$(wc -c < "$2" 2>/dev/null | tr -d ' ')
    now=$(date +%s)
    if [ "$now" -gt "$prev_t" ]; then speed=$(((have - prev) / (now - prev_t))); prev=$have; prev_t=$now; fi
    if [ -n "$total" ] && [ "$total" -gt 0 ]; then
      draw $((base + have * span / total)) "downloading $3  $(human "$have") / $(human "$total")  $(human "$speed")/s"
    else
      draw "$base" "downloading $3  $(human "$have")  $(human "$speed")/s"
    fi
  done
  wait "$pid" # curl의 종료 코드가 그대로 — 실패면 set -e로 여기서 멈춘다
  have=$(wc -c < "$2" | tr -d ' ')
  elapsed=$(($(date +%s) - start_t)); [ "$elapsed" -gt 0 ] || elapsed=1
  [ "${4:-}" = quiet ] || line "downloaded: $3 ($(human "$have"), $(human $((have / elapsed)))/s avg)"
}

step "detecting platform"
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
dest="${BRV_INSTALL_DIR:-$HOME/.local/bin}"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

step "downloading $asset"
fetch "$BASE/$asset" "$tmp/$asset" "$asset"
step "downloading SHA256SUMS"
fetch "$BASE/SHA256SUMS" "$tmp/SHA256SUMS" "SHA256SUMS" quiet

# 체크섬 검증 — sha256sum(리눅스) / shasum(macOS)
step "verifying checksum"
grep " $asset\$" "$tmp/SHA256SUMS" > "$tmp/sum.txt"
(
  cd "$tmp"
  if command -v sha256sum > /dev/null 2>&1; then
    sha256sum -c sum.txt > /dev/null
  else
    shasum -a 256 -c sum.txt > /dev/null
  fi
)
line "checksum verified"

step "installing to $dest"
tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$dest"
install -m 755 "$tmp/brv" "$dest/brv"
line "installed: $dest/brv — $("$dest/brv" --version)"

# PATH 자동 등록 (2026-09-02, 실측 UX: 새 서버 첫 설치마다 수동 추가를 요구하던 것을 흡수).
# rustup/uv 관행을 따른다: 마커 달린 한 줄을 셸 설정에 추가, 이미 있으면 건너뜀(멱등).
# 대상: ~/.profile(로그인 셸, 없으면 생성) + ~/.bashrc(있을 때) + ~/.zshrc(있거나 zsh 사용 시).
step "updating PATH"
case ":$PATH:" in
  *":$dest:"*) ;;
  *)
    if [ "${BRV_NO_MODIFY_PATH:-0}" = 1 ]; then
      line "note: $dest is not on your PATH. Add it to your shell config:"
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
        line "added $dest to PATH (${added# }) — new shells pick it up. For this shell right now:"
      else
        line "the PATH entry is already in your shell config — new shells pick it up. For this shell right now:"
      fi
      echo "  export PATH=\"$dest:\$PATH\""
      case "${SHELL:-}" in
        *fish*) echo "for fish: fish_add_path $dest" ;;
      esac
    fi ;;
esac

# 갱신이면 돌고 있는 서비스 데몬을 새 바이너리로 재기동 (2026-09-03, 사용자 지적 "갱신마다
# launchctl을 쳐야 하나") — 서비스가 없으면 조용히 지나간다 (brv daemon restart의 미등록 오류 숨김)
step "restarting the daemon if one is registered"
"$dest/brv" daemon restart 2>/dev/null || true
draw 100 "done"
[ "$tty" = 1 ] && printf '\n' >&2
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
