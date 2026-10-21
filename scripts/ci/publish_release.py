"""Verify release archives and add missing assets without changing published files."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
from urllib.parse import quote

from release_version import SEMVER

ARCHIVES = (
    "aarch64-apple-darwin.tar.gz", "aarch64-unknown-linux-gnu.tar.gz",
    "x86_64-apple-darwin.tar.gz", "x86_64-unknown-linux-gnu.tar.gz",
    "x86_64-pc-windows-msvc.zip", "web.zip",
)


def gh(*arguments):
    return subprocess.run(["gh", *map(str, arguments)], check=True, capture_output=True, text=True).stdout


def tag_version(tag):
    if not tag.startswith("v") or not SEMVER.fullmatch(tag[1:]):
        raise ValueError(f"Invalid release tag: {tag!r}")
    return tag[1:]


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def verified_assets(directory, tag):
    version = tag_version(tag)
    files = sorted(directory / f"restrust-{version}-{suffix}" for suffix in ARCHIVES)
    checksums = []
    for path in files:
        if not path.is_file() or not path.stat().st_size:
            raise ValueError(f"Missing release archive: {path.name}")
        expected = f"{digest(path)}  {path.name}"
        checksum = path.with_name(path.name + ".sha256")
        if not checksum.is_file() or checksum.read_text(encoding="utf-8").strip() != expected:
            raise ValueError(f"Checksum mismatch: {path.name}")
        checksums.append(expected)
    manifest = directory / "SHA256SUMS"
    manifest.write_text("\n".join(checksums) + "\n", encoding="utf-8")
    return [*files, manifest]


def get_release(repository, tag, command):
    try:
        return json.loads(command("api", f"repos/{repository}/releases/tags/{quote(tag, safe='')}"))
    except subprocess.CalledProcessError as error:
        # Authentication, rate-limit and network errors must not be treated as a missing release.
        if "(HTTP 404)" in (error.stderr or ""):
            return None
        raise


def publish_assets(repository, tag, directory, command=gh):
    files = verified_assets(directory, tag)
    release = get_release(repository, tag, command)
    prerelease = str(SEMVER.fullmatch(tag[1:]).group("pre") is not None).lower()
    if release is None:
        command("release", "create", tag, "--repo", repository, "--verify-tag", "--draft",
                "--title", tag, f"--prerelease={prerelease}", "--generate-notes", *files)
        return "Created a draft release with all verified assets."
    if release["draft"]:
        command("release", "upload", tag, "--repo", repository, *files, "--clobber")
        command("release", "edit", tag, "--repo", repository, f"--prerelease={prerelease}")
        return "Updated the draft release with all verified assets."

    existing = {asset["name"]: asset for asset in release["assets"]}
    missing = []
    # Validate every existing file before uploading any new ones. This also makes a
    # retry after a partial upload safe, without --clobber on a published release.
    with tempfile.TemporaryDirectory(prefix="restrust-existing-assets-") as temporary:
        for path in files:
            asset = existing.get(path.name)
            if asset is None:
                missing.append(path)
                continue
            remote_digest = asset.get("digest")
            if not remote_digest:
                command("release", "download", tag, "--repo", repository,
                        "--pattern", path.name, "--dir", temporary)
                remote_digest = "sha256:" + digest(Path(temporary) / path.name)
            if remote_digest != "sha256:" + digest(path):
                raise ValueError(f"Published asset differs: {path.name}. It will not be overwritten.")
    if missing:
        if release.get("immutable"):
            raise ValueError("This release is immutable; GitHub does not allow adding missing assets.")
        command("release", "upload", tag, "--repo", repository, *missing)
    return f"Uploaded {len(missing)} missing assets; matching published assets were preserved."


def verify_source_run(repository, tag, run_id, command=gh):
    tag_version(tag)
    if not str(run_id).isdigit() or int(run_id) < 1:
        raise ValueError("Source run ID must be a positive integer")
    run = json.loads(command("api", f"repos/{repository}/actions/runs/{run_id}"))
    commit = json.loads(command("api", f"repos/{repository}/commits/{quote('refs/tags/' + tag, safe='')}"))
    if not (
        run["event"] == "push" and run["head_branch"] == tag
        and run["head_sha"] == commit["sha"] and run["status"] == "completed"
        and run["path"] == ".github/workflows/release.yml"
        and run["head_repository"]["full_name"] == repository
    ):
        raise ValueError("Source run must be a completed Release run for this repository and tag commit")
    pages = json.loads(command("api", "--paginate", "--slurp",
                               f"repos/{repository}/actions/runs/{run_id}/jobs?per_page=100"))
    publishing = {"Create draft release", "Upload release assets"}
    jobs = [job for page in pages for job in page["jobs"] if job["name"] not in publishing]
    if len(jobs) < 8 or any(job["conclusion"] != "success" for job in jobs):
        raise ValueError("All version, test and build jobs must have succeeded before retrying the upload")
    return f"Verified source run {run_id} for {tag} at {commit['sha']}."


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", default=os.environ.get("RELEASE_TAG", os.environ.get("GITHUB_REF_NAME")))
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"))
    parser.add_argument("--dist", type=Path, default=Path("dist"))
    parser.add_argument("--verify-run", help="Validate a completed source run without uploading")
    args = parser.parse_args()
    if not args.tag or not args.repo:
        parser.error("Release tag and repository are required")
    try:
        if args.verify_run:
            print(verify_source_run(args.repo, args.tag, args.verify_run))
        else:
            print(publish_assets(args.repo, args.tag, args.dist))
    except (ValueError, subprocess.CalledProcessError) as error:
        message = error.stderr if isinstance(error, subprocess.CalledProcessError) else str(error)
        parser.exit(1, f"Release upload error: {message}\n")


if __name__ == "__main__":
    main()
