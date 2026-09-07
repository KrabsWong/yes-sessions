#!/usr/bin/env python3
"""Render the release cask without modifying a live tap or publishing artifacts."""
import argparse
import re
from pathlib import Path


def render(version: str, sha256: str) -> str:
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError("Expected a stable major.minor.patch version")
    if not re.fullmatch(r"[a-f0-9]{64}", sha256):
        raise ValueError("Expected a lowercase SHA256 checksum")
    template = Path(__file__).resolve().parent.parent / "packaging/yes-sessions.rb.in"
    return template.read_text().replace("@VERSION@", version).replace("@SHA256@", sha256)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("sha256")
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_text(render(args.version, args.sha256))
