"""Run a build command and include failures in GitHub's check annotations."""
import collections
import os
import re
import subprocess
import sys


def main():
    tail = collections.deque(maxlen=100)
    with subprocess.Popen(sys.argv[1:], stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                          text=True, encoding="utf-8", errors="replace") as process:
        for line in process.stdout:
            print(line, end="", flush=True)
            tail.append(line)
        code = process.wait()
    if code and os.environ.get("GITHUB_ACTIONS") == "true":
        message = re.sub(r"\x1b\[[0-9;]*m", "", "".join(tail))[-12000:]
        message = message.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
        print(f"::error title=Build or test failed::{message}")
    return code


if __name__ == "__main__":
    sys.exit(main())
