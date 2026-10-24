from pathlib import Path
import os
import subprocess
import sys
import tempfile
import tomllib
import unittest
import zipfile

from package_release import package
from release_version import ROOT, prepare_workspace, validate_tag, workspace_version


class ReleaseVersionTests(unittest.TestCase):
    def test_beta_tag_for_matching_base_version(self):
        self.assertEqual(validate_tag("v0.1.0-beta", "0.1.0"), ("0.1.0-beta", True))

    def test_stable_tag(self):
        self.assertEqual(validate_tag("v0.1.0", "0.1.0"), ("0.1.0", False))

    def test_numbered_prereleases_and_build_metadata(self):
        for version in ["0.1.0-alpha.1", "0.1.0-beta.2", "0.1.0-rc.1+build.7"]:
            with self.subTest(version=version):
                self.assertEqual(validate_tag(f"v{version}", "0.1.0"), (version, True))

    def test_explicit_cargo_prerelease_requires_exact_match(self):
        self.assertEqual(validate_tag("v0.1.0-beta", "0.1.0-beta"), ("0.1.0-beta", True))
        for tag in ["v0.1.0", "v0.1.0-beta.1", "v0.1.0-rc.1"]:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                validate_tag(tag, "0.1.0-beta")

    def test_different_base_version_is_rejected(self):
        with self.assertRaises(ValueError):
            validate_tag("v0.2.0-beta", "0.1.0")

    def test_invalid_tags_are_rejected(self):
        for tag in ["0.1.0-beta", "v0.1", "v01.1.0-beta", "v0.1.0-", "v0.1.0-beta.01",
                    "v0.1.0-beta/1", "v0.1.0-beta\nprerelease=false", "v0.1.0-beta+bad..meta"]:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                validate_tag(tag, "0.1.0")

    def test_cli_exports_release_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            current = workspace_version(ROOT)
            version = current if "-" in current else current + "-beta"
            output = Path(directory) / "outputs"
            result = subprocess.run(
                [sys.executable, str(Path(__file__).with_name("release_version.py"))],
                env={**os.environ, "GITHUB_REF_NAME": f"v{version}", "GITHUB_OUTPUT": str(output)},
                capture_output=True, text=True, check=True,
            )
            self.assertEqual(f"version={version}\nprerelease=true\n", result.stdout)
            self.assertEqual(output.read_text(), result.stdout)

    def test_prepared_workspace_builds_locked_with_tag_version_and_packages_it(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = root / "Cargo.toml"
            manifest.write_text(
                '[workspace]\nmembers=["app"]\nresolver="2"\n\n'
                '[workspace.package]\nversion = "0.1.0" # release version\n'
                '\n[workspace.metadata]\nversion = "do not change"\n'
            )
            source = root / "app" / "src"
            source.mkdir(parents=True)
            (source.parent / "Cargo.toml").write_text(
                '[package]\nname="release-version-fixture"\nversion.workspace=true\nedition="2021"\n'
            )
            (source / "main.rs").write_text('fn main() { println!("{}", env!("CARGO_PKG_VERSION")); }\n')
            prepare_workspace(root, "v0.1.0-beta")
            self.assertIn('version = "0.1.0-beta" # release version', manifest.read_text())
            self.assertIn('version = "do not change"', manifest.read_text())
            locked = tomllib.loads((root / "Cargo.lock").read_text())
            self.assertEqual(locked["package"][0]["version"], "0.1.0-beta")
            result = subprocess.run(
                ["cargo", "run", "--locked", "--quiet"], cwd=root,
                capture_output=True, text=True, check=True,
            )
            self.assertEqual(result.stdout.strip(), "0.1.0-beta")
            # Preparing again is a no-op, including for the generated lockfile.
            before = (manifest.read_bytes(), (root / "Cargo.lock").read_bytes())
            prepare_workspace(root, "v0.1.0-beta")
            self.assertEqual(before, (manifest.read_bytes(), (root / "Cargo.lock").read_bytes()))

            for name in ["README.md", "LICENSE-MIT", "LICENSE-APACHE"]:
                (root / name).write_text(name)
            (root / "docs").mkdir()
            web = root / "client" / "dist"
            web.mkdir(parents=True)
            (web / "index.html").write_text("<html></html>")
            (web / "client.js").write_text("// fixture")
            (web / "client.wasm").write_bytes(b"\0asm\x01\0\0\0")
            outputs = package(root, web=True)
            self.assertEqual([path.name for path in outputs], ["restrust-0.1.0-beta-web.zip"])
            with zipfile.ZipFile(outputs[0]) as archive:
                self.assertIn("restrust-0.1.0-beta-web/web/index.html", archive.namelist())

    def test_rejected_tag_does_not_modify_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = root / "Cargo.toml"
            original = '[workspace.package]\nversion="0.1.0"\n'
            manifest.write_text(original)
            with self.assertRaises(ValueError):
                prepare_workspace(root, "v0.2.0-beta")
            self.assertEqual(manifest.read_text(), original)


if __name__ == "__main__":
    unittest.main()
