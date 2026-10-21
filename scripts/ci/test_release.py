import hashlib
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile

from package_release import archive_directory, package


class ReleaseTests(unittest.TestCase):
    def test_archives_preserve_content_checksums_and_unix_modes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            staging = root / "restrust-test"
            staging.mkdir()
            binary = staging / "restrust-client"
            binary.write_bytes(b"test executable")
            binary.chmod(0o755)
            for windows, suffix in [(False, ".tar.gz"), (True, ".zip")]:
                output = archive_directory(staging, root / ("release" + suffix), windows)
                checksum = output.with_name(output.name + ".sha256").read_text()
                self.assertEqual(checksum, f"{hashlib.sha256(output.read_bytes()).hexdigest()}  {output.name}\n")
                if windows:
                    with zipfile.ZipFile(output) as archive:
                        self.assertEqual(archive.read("restrust-test/restrust-client"), binary.read_bytes())
                else:
                    with tarfile.open(output) as archive:
                        entry = archive.getmember("restrust-test/restrust-client")
                        self.assertEqual(entry.mode & 0o111, 0o111)
                        self.assertEqual(archive.extractfile(entry).read(), binary.read_bytes())

    def test_missing_release_binary_fails_instead_of_uploading_an_empty_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[workspace.package]\nversion="0.1.0"\n')
            with self.assertRaisesRegex(AssertionError, "Missing release binary"):
                package(root, "x86_64-pc-windows-msvc")


if __name__ == "__main__":
    unittest.main()
