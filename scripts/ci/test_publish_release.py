import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from publish_release import ARCHIVES, DEB_ARCHITECTURES, deb_name, digest, publish_assets, verified_assets, verify_source_run

REPOSITORY = "owner/restrust"
TAG = "v0.1.0-beta"


class FakeGitHub:
    def __init__(self, release):
        self.release = release
        self.calls = []
        self.downloads = {}

    def __call__(self, *arguments):
        args = list(map(str, arguments))
        self.calls.append(args)
        if args[0] == "api":
            if self.release is None:
                raise subprocess.CalledProcessError(1, args, stderr="gh: Not Found (HTTP 404)")
            return json.dumps(self.release)
        if args[1] == "download":
            name = args[args.index("--pattern") + 1]
            destination = Path(args[args.index("--dir") + 1])
            (destination / name).write_bytes(self.downloads[name])
        if args[1] == "upload":
            for argument in args[5:]:
                if argument == "--clobber":
                    continue
                path = Path(argument)
                self.release["assets"].append({"name": path.name, "digest": "sha256:" + digest(path)})
        return ""

    def writes(self):
        return [args for args in self.calls if args[0] == "release" and args[1] in {"create", "edit", "upload"}]


class PublishReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        names = [f"restrust-0.1.0-beta-{suffix}" for suffix in ARCHIVES]
        names += [deb_name("0.1.0-beta", architecture) for architecture in DEB_ARCHITECTURES]
        for name in names:
            path = self.directory / name
            path.write_bytes(f"archive for {name}".encode())
            path.with_name(path.name + ".sha256").write_text(f"{digest(path)}  {path.name}\n")

    def test_existing_published_prerelease_receives_all_assets(self):
        github = FakeGitHub({"draft": False, "prerelease": True, "assets": []})
        publish_assets(REPOSITORY, TAG, self.directory, github)
        writes = github.writes()
        self.assertEqual(len(writes), 1)
        self.assertEqual(writes[0][:3], ["release", "upload", TAG])
        self.assertNotIn("--clobber", writes[0])
        self.assertEqual(len(github.release["assets"]), 9)
        self.assertEqual(github.release["draft"], False)
        self.assertEqual(github.release["prerelease"], True)

    def test_retry_skips_all_matching_published_assets(self):
        github = FakeGitHub({"draft": False, "assets": []})
        publish_assets(REPOSITORY, TAG, self.directory, github)
        github.calls.clear()
        publish_assets(REPOSITORY, TAG, self.directory, github)
        self.assertEqual(github.writes(), [])

    def test_partial_upload_only_adds_missing_files(self):
        files = verified_assets(self.directory, TAG)
        existing = files[0]
        github = FakeGitHub({"draft": False, "assets": [{"name": existing.name, "digest": "sha256:" + digest(existing)}]})
        publish_assets(REPOSITORY, TAG, self.directory, github)
        self.assertNotIn(str(existing), github.writes()[0])
        self.assertEqual(len(github.release["assets"]), 9)

    def test_different_published_file_blocks_all_uploads(self):
        existing = verified_assets(self.directory, TAG)[-1]
        github = FakeGitHub({"draft": False, "assets": [{"name": existing.name, "digest": "sha256:wrong"}]})
        with self.assertRaisesRegex(ValueError, "Published asset differs"):
            publish_assets(REPOSITORY, TAG, self.directory, github)
        self.assertEqual(github.writes(), [])

    def test_older_assets_without_digest_are_downloaded_and_verified(self):
        existing = verified_assets(self.directory, TAG)[0]
        github = FakeGitHub({"draft": False, "assets": [{"name": existing.name}]})
        github.downloads[existing.name] = existing.read_bytes()
        publish_assets(REPOSITORY, TAG, self.directory, github)
        self.assertNotIn(str(existing), github.writes()[0])
        self.assertTrue(any(call[:2] == ["release", "download"] for call in github.calls))

    def test_new_release_is_still_a_draft_prerelease(self):
        github = FakeGitHub(None)
        publish_assets(REPOSITORY, TAG, self.directory, github)
        create = github.writes()[0]
        self.assertEqual(create[:3], ["release", "create", TAG])
        self.assertIn("--draft", create)
        self.assertIn("--prerelease=true", create)

    def test_new_stable_release_is_not_marked_as_prerelease(self):
        for path in list(self.directory.iterdir()):
            renamed = path.with_name(path.name.replace("0.1.0-beta", "0.1.0"))
            path.rename(renamed)
            if renamed.name.endswith(".sha256"):
                renamed.write_text(renamed.read_text().replace("0.1.0-beta", "0.1.0"))
        github = FakeGitHub(None)
        publish_assets(REPOSITORY, "v0.1.0", self.directory, github)
        self.assertIn("--prerelease=false", github.writes()[0])

    def test_existing_draft_can_replace_assets(self):
        github = FakeGitHub({"draft": True, "assets": []})
        publish_assets(REPOSITORY, TAG, self.directory, github)
        self.assertIn("--clobber", github.writes()[0])
        self.assertIn("--prerelease=true", github.writes()[1])

    def test_immutable_release_reports_missing_assets(self):
        github = FakeGitHub({"draft": False, "immutable": True, "assets": []})
        with self.assertRaisesRegex(ValueError, "immutable"):
            publish_assets(REPOSITORY, TAG, self.directory, github)
        self.assertEqual(github.writes(), [])

    def test_invalid_or_incomplete_downloads_never_reach_github(self):
        files = verified_assets(self.directory, TAG)
        files[0].write_bytes(b"corrupt archive")
        github = FakeGitHub(None)
        with self.assertRaisesRegex(ValueError, "Checksum mismatch"):
            publish_assets(REPOSITORY, TAG, self.directory, github)
        self.assertEqual(github.calls, [])
        files[0].unlink()
        with self.assertRaisesRegex(ValueError, "Missing release archive"):
            publish_assets(REPOSITORY, TAG, self.directory, github)
        self.assertEqual(github.calls, [])

    def test_api_failure_does_not_create_a_release(self):
        calls = []
        def unauthorized(*args):
            calls.append(args)
            raise subprocess.CalledProcessError(1, args, stderr="gh: Unauthorized (HTTP 401)")
        with self.assertRaises(subprocess.CalledProcessError):
            publish_assets(REPOSITORY, TAG, self.directory, unauthorized)
        self.assertEqual(len(calls), 1)


class RecoveryRunTests(unittest.TestCase):
    def setUp(self):
        self.run = {"event": "push", "head_branch": TAG, "head_sha": "tag-commit", "status": "completed",
                    "path": ".github/workflows/release.yml", "head_repository": {"full_name": REPOSITORY}}
        self.jobs = [{"name": f"build {number}", "conclusion": "success"} for number in range(8)]
        self.jobs.append({"name": "Create draft release", "conclusion": "failure"})

    def command(self, *args):
        endpoint = args[-1]
        if "/commits/" in endpoint:
            return json.dumps({"sha": "tag-commit"})
        if "/jobs?" in endpoint:
            return json.dumps([{"jobs": self.jobs}])
        return json.dumps(self.run)

    def test_failed_publisher_with_successful_builds_can_be_recovered(self):
        self.assertIn("Verified source run", verify_source_run(REPOSITORY, TAG, "123", self.command))

    def test_wrong_tag_commit_or_untrusted_run_is_rejected(self):
        for key, value in [("head_sha", "other-commit"), ("event", "pull_request"),
                           ("head_branch", "main"), ("status", "in_progress"),
                           ("path", ".github/workflows/ci.yml"),
                           ("head_repository", {"full_name": "other/repository"})]:
            with self.subTest(key=key):
                original = self.run[key]
                self.run[key] = value
                with self.assertRaisesRegex(ValueError, "Source run must"):
                    verify_source_run(REPOSITORY, TAG, "123", self.command)
                self.run[key] = original

    def test_failed_or_missing_builds_are_rejected(self):
        self.jobs[0]["conclusion"] = "failure"
        with self.assertRaisesRegex(ValueError, "must have succeeded"):
            verify_source_run(REPOSITORY, TAG, "123", self.command)
        self.jobs.pop(0)
        with self.assertRaisesRegex(ValueError, "must have succeeded"):
            verify_source_run(REPOSITORY, TAG, "123", self.command)


if __name__ == "__main__":
    unittest.main()
