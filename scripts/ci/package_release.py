"""Package native binaries or Trunk output, verify contents, and write SHA-256."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tarfile
import tempfile
import time
import tomllib
import urllib.error
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[2]


def smoke_server(binary, version):
    result = subprocess.run([str(binary), "--version"], check=True, capture_output=True, text=True)
    assert result.stdout.strip() == f"restrust-server {version}", result.stdout
    with tempfile.TemporaryDirectory(prefix="restrust-release-") as directory:
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        env = {k: v for k, v in os.environ.items() if not k.startswith("RESTRUST_")}
        env.update(RESTRUST_BIND_ADDR="127.0.0.1", RESTRUST_HTTP_PORT=str(port),
                   RESTRUST_DATA_DIR=directory, RESTRUST_TLS_MODE="none")
        with tempfile.TemporaryFile() as log:
            process = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 30
                while process.poll() is None and time.monotonic() < deadline:
                    try:
                        with urllib.request.urlopen(f"http://127.0.0.1:{port}/api/v1/healthz", timeout=1) as response:
                            data = json.load(response)
                        assert data["status"] == "ok" and data["version"] == version, data
                        return
                    except (urllib.error.URLError, TimeoutError):
                        time.sleep(0.1)
                log.seek(0)
                raise RuntimeError("Release server failed its health check:\n" + log.read().decode(errors="replace"))
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def archive_directory(staging, output, windows=False):
    """Archive one top-level directory, retaining Unix executable permissions."""
    output.parent.mkdir(parents=True, exist_ok=True)
    if windows:
        with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
            for source in sorted(staging.rglob("*")):
                if source.is_file():
                    archive.write(source, source.relative_to(staging.parent))
        with zipfile.ZipFile(output) as archive:
            assert archive.testzip() is None, "Corrupt release ZIP"
    else:
        with tarfile.open(output, "w:gz") as archive:
            archive.add(staging, arcname=staging.name)
        with tarfile.open(output) as archive:
            assert archive.getnames(), "Empty release archive"
    with output.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    output.with_name(output.name + ".sha256").write_text(f"{digest}  {output.name}\n", encoding="utf-8")
    return output


def package(root, target=None, web=False):
    with (root / "Cargo.toml").open("rb") as source:
        version = tomllib.load(source)["workspace"]["package"]["version"]
    name = f"restrust-{version}-{'web' if web else target}"
    windows = bool(target and "windows" in target)
    suffix = ".zip" if windows or web else ".tar.gz"
    with tempfile.TemporaryDirectory(prefix="restrust-package-") as directory:
        staging = Path(directory) / name
        staging.mkdir()
        if web:
            source = root / "client" / "dist"
            assert (source / "index.html").is_file(), "Missing Trunk index.html"
            wasm = list(source.glob("*.wasm"))
            assert wasm and list(source.glob("*.js")), "Missing Trunk WASM or JavaScript"
            for path in wasm:
                with path.open("rb") as binary:
                    assert binary.read(4) == b"\0asm", f"Invalid WASM: {path}"
            shutil.copytree(source, staging / "web")
        else:
            extension = ".exe" if windows else ""
            source = root / "target" / target / "release"
            for executable in ["restrust-client", "restrust-server"]:
                binary = source / (executable + extension)
                assert binary.is_file() and binary.stat().st_size, f"Missing release binary: {binary}"
                shutil.copy2(binary, staging / binary.name)
            smoke_server(staging / ("restrust-server" + extension), version)
        for filename in ["README.md", "LICENSE-MIT", "LICENSE-APACHE"]:
            shutil.copy2(root / filename, staging / filename)
        shutil.copytree(root / "docs", staging / "docs")
        return archive_directory(staging, root / "dist" / (name + suffix), windows or web)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--target")
    group.add_argument("--web", action="store_true")
    args = parser.parse_args()
    print(package(ROOT, args.target, args.web))
