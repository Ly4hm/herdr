import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from scripts import fork_sync


class ForkSyncTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.upstream = self.root / "upstream"
        self.upstream.mkdir()
        fork_sync.git(self.upstream, "init", "-b", "master")
        self.configure(self.upstream)
        self.commit_file(self.upstream, "shared.txt", "original\n")
        fork_sync.git(self.upstream, "tag", "v1.0.0")
        self.repo = self.root / "fork"
        subprocess.run(
            ["git", "clone", "--quiet", str(self.upstream), str(self.repo)], check=True
        )
        self.configure(self.repo)
        fork_sync.git(self.repo, "checkout", "-b", fork_sync.BRANCH)

    @staticmethod
    def configure(repo):
        fork_sync.git(repo, "config", "user.name", "Fork sync test")
        fork_sync.git(repo, "config", "user.email", "test@example.invalid")

    @staticmethod
    def commit_file(repo, name, value):
        (repo / name).write_text(value)
        fork_sync.git(repo, "add", name)
        fork_sync.git(repo, "commit", "-m", "test: update fixture")

    def head(self, repo):
        return fork_sync.git(repo, "rev-parse", "HEAD").stdout.strip()

    def test_existing_release_does_not_change_history(self):
        previous = self.head(self.repo)
        self.assertFalse(fork_sync.prepare_merge(self.repo, "v1.0.0", str(self.upstream)))
        self.assertEqual(self.head(self.repo), previous)

    def test_new_release_merges_and_preserves_personal_commits_without_remote_writes(self):
        self.commit_file(self.repo, "personal.txt", "mouse path support\n")
        previous = self.head(self.repo)
        self.commit_file(self.upstream, "official.txt", "official update\n")
        fork_sync.git(self.upstream, "tag", "v1.0.1")
        upstream_head = self.head(self.upstream)
        self.assertTrue(fork_sync.prepare_merge(self.repo, "v1.0.1", str(self.upstream)))
        self.assertEqual((self.repo / "personal.txt").read_text(), "mouse path support\n")
        self.assertEqual((self.repo / "official.txt").read_text(), "official update\n")
        self.assertEqual(fork_sync.git(self.repo, "rev-parse", "HEAD^1").stdout.strip(), previous)
        self.assertEqual(fork_sync.git(self.repo, "rev-parse", "HEAD^2").stdout.strip(), upstream_head)
        self.assertEqual(self.head(self.upstream), upstream_head)
        self.assertNotIn("v1.0.1", fork_sync.git(self.repo, "tag", "--list").stdout)
        self.assertFalse(fork_sync.prepare_merge(self.repo, "v1.0.1", str(self.upstream)))

    def test_conflict_aborts_and_preserves_personal_branch(self):
        self.commit_file(self.repo, "shared.txt", "personal change\n")
        previous = self.head(self.repo)
        self.commit_file(self.upstream, "shared.txt", "conflicting official change\n")
        fork_sync.git(self.upstream, "tag", "v1.0.1")
        with self.assertRaisesRegex(RuntimeError, "nothing was pushed"):
            fork_sync.prepare_merge(self.repo, "v1.0.1", str(self.upstream))
        self.assertEqual(self.head(self.repo), previous)
        self.assertEqual((self.repo / "shared.txt").read_text(), "personal change\n")
        self.assertEqual(fork_sync.git(self.repo, "status", "--porcelain").stdout, "")

    def test_wrong_branch_and_dirty_checkout_are_rejected(self):
        fork_sync.git(self.repo, "checkout", "master")
        with self.assertRaisesRegex(ValueError, "checkout must be on"):
            fork_sync.prepare_merge(self.repo, "v1.0.0", str(self.upstream))
        fork_sync.git(self.repo, "checkout", fork_sync.BRANCH)
        (self.repo / "shared.txt").write_text("uncommitted\n")
        with self.assertRaisesRegex(ValueError, "must be clean"):
            fork_sync.prepare_merge(self.repo, "v1.0.0", str(self.upstream))

    def test_tag_input_cannot_select_preview_or_inject_git_options(self):
        for tag in ["master", "v1.0.0-preview", "v1.0.0\n", "--all", "v1.0.0; echo nope", "v1/0/0"]:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                fork_sync.stable_tag(tag)
        self.assertEqual(fork_sync.stable_tag("v1.2.3"), "v1.2.3")

    def test_release_api_rejects_prereleases(self):
        release = {"tag_name": "v1.0.1", "draft": False, "prerelease": True}
        with patch("urllib.request.urlopen", return_value=io.BytesIO(json.dumps(release).encode())):
            with self.assertRaisesRegex(ValueError, "published stable"):
                fork_sync.latest_stable_tag()

    def test_build_toolchains_follow_merged_sources(self):
        (self.repo / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.99.0"\n')
        vendor = self.repo / "vendor/libghostty-vt"
        vendor.mkdir(parents=True)
        (vendor / "build.zig.zon").write_text('.{ .minimum_zig_version = "0.16.0" }')
        self.assertEqual(fork_sync.build_versions(self.repo), ("1.99.0", "0.16.0"))


if __name__ == "__main__":
    unittest.main()
