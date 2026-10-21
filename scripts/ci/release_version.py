"""Validate release tags and prepare the workspace version for tagged CI builds."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[2]
NUMBER = r"(?:0|[1-9][0-9]*)"
IDENTIFIER = rf"(?:{NUMBER}|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)"
SEMVER = re.compile(
    rf"(?P<base>{NUMBER}\.{NUMBER}\.{NUMBER})"
    rf"(?:-(?P<pre>{IDENTIFIER}(?:\.{IDENTIFIER})*))?"
    r"(?:\+(?P<build>[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
)


def validate_tag(tag, workspace_version):
    version = tag.removeprefix("v")
    release = SEMVER.fullmatch(version)
    workspace = SEMVER.fullmatch(workspace_version)
    if not tag.startswith("v") or release is None or workspace is None:
        raise ValueError(f"Expected a SemVer tag such as v{workspace_version}-beta.1; got {tag!r}")
    # A stable workspace version may acquire a release-channel suffix in CI.
    # An explicitly versioned prerelease must retain its exact version.
    if version != workspace_version and not (
        workspace.group("pre") is None
        and workspace.group("build") is None
        and release.group("base") == workspace_version
        and release.group("pre") is not None
    ):
        raise ValueError(f"Tag {tag!r} must match Cargo version {workspace_version} or its prerelease")
    return version, release.group("pre") is not None


def workspace_version(root):
    return tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]


def prepare_workspace(root, tag):
    current = workspace_version(root)
    version, prerelease = validate_tag(tag, current)
    if version == current:
        return version, prerelease
    manifest = root / "Cargo.toml"
    original = manifest.read_text(encoding="utf-8")
    section = re.search(r"(?ms)^\[workspace\.package\][^\n]*\n(?P<body>.*?)(?=^\[|\Z)", original)
    if section is None:
        raise ValueError("Missing [workspace.package] table")
    body, count = re.subn(
        r'''(?m)^(\s*version\s*=\s*)(?:"[^"\n]*"|'[^'\n]*')''',
        lambda match: match[1] + json.dumps(version),
        section["body"],
    )
    if count != 1:
        raise ValueError("Expected one workspace.package.version")
    manifest.write_text(original[:section.start("body")] + body + original[section.end("body"):], encoding="utf-8")
    # Preserve locked third-party versions while refreshing workspace entries.
    subprocess.run(["cargo", "update", "--workspace"], cwd=root, check=True)
    return version, prerelease


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", default=os.environ.get("GITHUB_REF_NAME"))
    parser.add_argument("--prepare", action="store_true", help="Apply the tag version to this CI checkout")
    args = parser.parse_args()
    if not args.tag:
        parser.error("A release tag is required")
    try:
        result = prepare_workspace(ROOT, args.tag) if args.prepare else validate_tag(args.tag, workspace_version(ROOT))
    except ValueError as error:
        parser.exit(1, f"Release version error: {error}\n")
    version, prerelease = result
    outputs = f"version={version}\nprerelease={str(prerelease).lower()}\n"
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            output.write(outputs)
    print(outputs, end="")


if __name__ == "__main__":
    main()
