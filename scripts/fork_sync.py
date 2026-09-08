#!/usr/bin/env python3
"""Prepare a stable-upstream merge locally; CI validates it before any push."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib
import urllib.request


BRANCH = "feature/local-path-click"
UPSTREAM = "https://github.com/herdrdev/herdr.git"
LATEST_RELEASE = "https://api.github.com/repos/herdrdev/herdr/releases/latest"


def stable_tag(value):
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", value):
        raise ValueError("upstream tag must have the form vMAJOR.MINOR.PATCH")
    return value


def latest_stable_tag():
    request = urllib.request.Request(
        LATEST_RELEASE,
        headers={"Accept": "application/vnd.github+json", "User-Agent": "Ly4hm-herdr-fork-sync"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        release = json.load(response)
    if release.get("draft") or release.get("prerelease"):
        raise ValueError("latest release is not a published stable release")
    return stable_tag(release["tag_name"])


def git(repo, *args, check=True):
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        check=check,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def prepare_merge(repo, tag, upstream=UPSTREAM):
    """Fetch one upstream tag and merge it; never write to any remote."""
    tag = stable_tag(tag)
    if git(repo, "branch", "--show-current").stdout.strip() != BRANCH:
        raise ValueError(f"checkout must be on {BRANCH}")
    if git(repo, "status", "--porcelain").stdout.strip():
        raise ValueError("checkout must be clean before preparing a merge")
    # Fetch only the requested tag into FETCH_HEAD, avoiding local tag conflicts
    # and avoiding a remote named upstream that could point somewhere else.
    git(repo, "fetch", "--no-tags", upstream, f"refs/tags/{tag}")
    upstream_commit = git(repo, "rev-parse", "FETCH_HEAD^{commit}").stdout.strip()
    ancestor = git(repo, "merge-base", "--is-ancestor", upstream_commit, "HEAD", check=False)
    if ancestor.returncode == 0:
        return False
    if ancestor.returncode != 1:
        raise RuntimeError(ancestor.stderr.strip())
    merged = git(
        repo, "merge", "--no-ff", "-m", f"chore: merge upstream {tag}",
        upstream_commit, check=False,
    )
    if merged.returncode:
        git(repo, "merge", "--abort", check=False)
        raise RuntimeError(f"upstream merge failed; nothing was pushed:\n{merged.stdout}{merged.stderr}")
    return True


def build_versions(repo):
    with (repo / "rust-toolchain.toml").open("rb") as source:
        rust = tomllib.load(source)["toolchain"]["channel"]
    if not re.fullmatch(r"[a-zA-Z0-9.+-]+", rust):
        raise ValueError("invalid Rust toolchain channel")
    zig_config = (repo / "vendor/libghostty-vt/build.zig.zon").read_text()
    zig = re.search(r'\.minimum_zig_version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"', zig_config)
    if zig is None:
        raise ValueError("cannot determine vendored Ghostty Zig version")
    return rust, zig.group(1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", default="", help="stable upstream tag; defaults to latest release")
    parser.add_argument("--validate-always", action="store_true")
    args = parser.parse_args()
    repo = Path.cwd()
    tag = stable_tag(args.tag) if args.tag else latest_stable_tag()
    changed = prepare_merge(repo, tag)
    rust, zig = build_versions(repo)
    results = {
        "tag": tag,
        "changed": str(changed).lower(),
        "validate": str(changed or args.validate_always).lower(),
        "rust": rust,
        "zig": zig,
    }
    print(json.dumps(results, indent=2))
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a", encoding="utf-8") as target:
            for key, value in results.items():
                target.write(f"{key}={value}\n")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        if isinstance(error, subprocess.CalledProcessError):
            print(error.stderr)
        raise SystemExit(str(error)) from error
