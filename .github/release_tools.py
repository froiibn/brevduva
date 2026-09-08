# Copyright 2026 SEIZIA (Jaeyoung Ko)
# SPDX-License-Identifier: Apache-2.0
"""릴리스 검증: 성공한 정확한 커밋만, 모든 자산을 준비한 뒤 공개한다."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time
import tomllib

ASSETS = {
    "brv-aarch64-apple-darwin.tar.gz", "brv-x86_64-apple-darwin.tar.gz",
    "brv-aarch64-unknown-linux-musl.tar.gz", "brv-x86_64-unknown-linux-musl.tar.gz",
    "brv-x86_64-pc-windows-msvc.zip",
}


def run(*args):
    return subprocess.check_output(args, text=True, encoding="utf-8").strip()


def api(path):
    return json.loads(run("gh", "api", path))


def manifest_version(contents):
    # 기존 Cargo.toml은 UTF-8 BOM을 포함한다. 경로 읽기와 git show 모두 같은 규칙을 쓴다.
    return tomllib.loads(contents.removeprefix("\ufeff"))["workspace"]["package"]["version"]


def successful_run(repo, sha, workflow="ci.yml", event="push"):
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("정확한 40자리 커밋 SHA가 필요합니다")
    runs = api(f"repos/{repo}/actions/workflows/{workflow}/runs?head_sha={sha}&event={event}&per_page=100")["workflow_runs"]
    runs = [r for r in runs if r["head_sha"] == sha and r["event"] == event]
    if not runs:
        raise ValueError(f"{repo}: 해당 커밋의 CI 결과가 없습니다")
    latest = max(runs, key=lambda r: r["id"])
    if latest["status"] != "completed" or latest["conclusion"] != "success":
        raise ValueError(f"{repo}: 최신 실행이 통과하지 않았습니다: {latest['html_url']}")
    return latest


def validate_assets(directory):
    files = {p.name for p in directory.iterdir() if p.is_file()}
    if files - {"SHA256SUMS"} != ASSETS or any(p.is_dir() for p in directory.iterdir()):
        raise ValueError("설치 파일 5종이 정확히 준비되지 않았습니다")
    if any((directory / name).stat().st_size == 0 for name in ASSETS):
        raise ValueError("빈 설치 파일이 있습니다")
    return "".join(f"{hashlib.sha256((directory / name).read_bytes()).hexdigest()}  {name}\n" for name in sorted(ASSETS))


def retry_download(repo, run_id, directory):
    # 일시적인 자산 다운로드 장애만 제한적으로 재시도한다. 테스트 실패는 재시도하지 않는다.
    for attempt in range(3):
        try:
            run("gh", "run", "download", str(run_id), "-R", repo, "--pattern", "brv-*", "--dir", str(directory))
            # gh는 아티팩트별 디렉터리에 푼다. 설치 파일만 같은 dist에 모은다.
            for folder in list(directory.iterdir()):
                if folder.is_dir():
                    for file in folder.iterdir():
                        if not file.is_file() or file.name not in ASSETS:
                            raise ValueError("예상하지 않은 빌드 자산")
                        file.replace(directory / file.name)
                    folder.rmdir()
            return
        except subprocess.CalledProcessError:
            if attempt == 2:
                raise
            # 전용 임시 디렉터리의 부분 다운로드만 제거한다.
            import shutil
            shutil.rmtree(directory)
            directory.mkdir()
            time.sleep(5 * (attempt + 1))


def find_release(repo, tag):
    pages = json.loads(run("gh", "api", "--paginate", "--slurp", f"repos/{repo}/releases?per_page=100"))
    return next((r for page in pages for r in page if r["tag_name"] == tag), None)


def publish(repo, tag, directory):
    checksums = validate_assets(directory)
    (directory / "SHA256SUMS").write_text(checksums)
    # API 실패를 "릴리스가 없다"로 오인하지 않는다. 목록 조회 실패는 그대로 중단한다.
    existing = find_release(repo, tag)
    if existing and not existing["draft"]:
        raise ValueError("이미 공개된 릴리스는 덮어쓰지 않습니다")
    if not existing:
        notes = Path("docs") / f"RELEASE_{tag[1:]}.md"
        if not notes.is_file():
            raise ValueError("버전에 맞는 릴리스 노트가 필요합니다")
        run("gh", "release", "create", tag, "-R", repo, "--draft", "--verify-tag", "--title", f"brv {tag}", "--notes-file", str(notes))
    run("gh", "release", "upload", tag, "-R", repo, "--clobber", *(str(directory / name) for name in sorted(ASSETS | {"SHA256SUMS"})))
    # 원격 업로드도 실제로 다시 받아 해시 대조 후 공개한다.
    import tempfile
    with tempfile.TemporaryDirectory() as temp:
        run("gh", "release", "download", tag, "-R", repo, "--dir", temp)
        remote = Path(temp)
        if validate_assets(remote) != checksums or (remote / "SHA256SUMS").read_text() != checksums:
            raise ValueError("업로드한 설치 파일의 해시가 다릅니다")
    run("gh", "release", "edit", tag, "-R", repo, "--draft=false", "--latest")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["check", "publish"])
    args = parser.parse_args()
    repo, sha, tag = (os.environ[k] for k in ["GITHUB_REPOSITORY", "GITHUB_SHA", "GITHUB_REF_NAME"])
    version = manifest_version(Path("Cargo.toml").read_text(encoding="utf-8"))
    if os.environ.get("GITHUB_REF_TYPE") != "tag" or tag != "v" + version:
        raise ValueError("버전과 일치하는 태그에서만 릴리스할 수 있습니다")
    successful_run(repo, sha)
    if args.action == "publish":
        import tempfile
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            retry_download(repo, os.environ["GITHUB_RUN_ID"], directory)
            publish(repo, tag, directory)


if __name__ == "__main__":
    main()
