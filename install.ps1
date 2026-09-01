# Copyright 2026 SEIZIA (Jaeyoung Ko) — SPDX-License-Identifier: Apache-2.0
# Brevduva 리시버(brv) 설치 — Windows x86_64
#
#   irm https://brevduva.dev/install.ps1 | iex
#
# 하는 일: GitHub Releases 최신 버전에서 바이너리를 받아 SHA256 검증 후
# %USERPROFILE%\.local\bin\brv.exe 로 설치하고(BRV_INSTALL_DIR로 변경 가능),
# 그 경로가 사용자 PATH에 없으면 추가한다. 그 외에는 아무것도 건드리지 않는다.
$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$repo = "froiibn/brevduva"
$base = "https://github.com/$repo/releases/latest/download"
$asset = "brv-x86_64-pc-windows-msvc.zip"

if ($env:BRV_INSTALL_DIR) { $dest = $env:BRV_INSTALL_DIR } else { $dest = Join-Path $env:USERPROFILE ".local\bin" }
$tmp = Join-Path $env:TEMP ("brv-install-" + [guid]::NewGuid().ToString("n"))
New-Item -ItemType Directory -Force $tmp | Out-Null

try {
    Write-Host "다운로드: $base/$asset"
    Invoke-WebRequest "$base/$asset" -OutFile "$tmp\$asset" -UseBasicParsing
    Invoke-WebRequest "$base/SHA256SUMS" -OutFile "$tmp\SHA256SUMS" -UseBasicParsing

    $line = Select-String -Path "$tmp\SHA256SUMS" -Pattern ([regex]::Escape($asset)) | Select-Object -First 1
    if (-not $line) { throw "SHA256SUMS에 $asset 항목이 없습니다" }
    $expected = ($line.Line -split '\s+')[0].ToLower()
    $actual = (Get-FileHash "$tmp\$asset" -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) { throw "체크섬 불일치 — 다운로드가 손상되었거나 변조되었습니다" }
    Write-Host "체크섬 확인 완료"

    Expand-Archive "$tmp\$asset" -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Force $dest | Out-Null
    Copy-Item "$tmp\brv.exe" (Join-Path $dest "brv.exe") -Force
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$exe = Join-Path $dest "brv.exe"
Write-Host "설치 완료: $exe — $(& $exe --version)"

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($userPath -split ';') -notcontains $dest) {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$dest", "User")
    Write-Host "PATH에 $dest 를 추가했습니다 — 새 터미널부터 적용됩니다"
}

Write-Host ""
Write-Host "다음 단계 — 계정과 이 머신을 연결하세요:"
Write-Host "  1) https://brevduva.dev 대시보드 → 머신 연결에서 등록 코드 발급 (brvenr_ 로 시작)"
Write-Host "  2) brv init --server https://api.brevduva.dev --enroll <코드>"
