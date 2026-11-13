#!/usr/bin/env python3
"""Download and validate the Silesia corpus used by the official LZ4 benchmark."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import zipfile

BASE = Path(__file__).resolve().parent
URL = "https://sun.aei.polsl.pl/~sdeor/corpus/silesia.zip"
# Pinned from the author's HTTPS archive downloaded on 2026-09-27.
ARCHIVE_SHA256 = "0626e25f45c0ffb5dc801f13b7c82a3b75743ba07e3a71835a41e3d9f63c77af"
# Sizes published by the corpus author. ZIP CRCs are checked during extraction.
FILES = {
    "dickens": 10192446, "mozilla": 51220480, "mr": 9970564,
    "nci": 33553445, "ooffice": 6152192, "osdb": 10085684,
    "reymont": 6627202, "samba": 21606400, "sao": 7251944,
    "webster": 41458703, "xml": 5345280, "x-ray": 8474240,
}


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", type=Path, default=BASE / "data")
    args = parser.parse_args()
    directory = args.data_dir.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    archive = directory / "silesia.zip"
    if not archive.exists():
        with tempfile.TemporaryDirectory(prefix="download-", dir=directory) as tmp:
            partial = Path(tmp) / "silesia.zip"
            subprocess.run(["curl", "--fail", "--location", "--retry", "3",
                            "--connect-timeout", "30", "--output", str(partial), URL],
                           check=True)
            if sha256(partial) != ARCHIVE_SHA256:
                raise ValueError("Downloaded archive does not match the pinned SHA-256")
            with zipfile.ZipFile(partial) as package:
                if package.testzip() is not None:
                    raise ValueError("Corrupt Silesia archive")
            partial.replace(archive)
    digest = sha256(archive)
    if digest != ARCHIVE_SHA256:
        raise ValueError(f"Silesia archive SHA-256 mismatch: {digest}")
    manifest = {"source": URL, "archive_sha256": digest, "files": {}}
    corpus = directory / "silesia"
    corpus.mkdir(exist_ok=True)
    with zipfile.ZipFile(archive) as package:
        for name, size in FILES.items():
            info = package.getinfo(name)
            if info.file_size != size:
                raise ValueError(f"Unexpected size for {name}: {info.file_size}")
            # Extract only known flat filenames, never arbitrary archive paths.
            with package.open(info) as source, tempfile.NamedTemporaryFile(
                    dir=corpus, prefix=f".{name}-", delete=False) as target:
                temporary = Path(target.name)
                try:
                    shutil.copyfileobj(source, target)
                except BaseException:
                    temporary.unlink()
                    raise
            temporary.replace(corpus / name)
            manifest["files"][name] = {"bytes": size, "sha256": sha256(corpus / name)}
    (directory / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Validated {len(FILES)} files, {sum(FILES.values()):,} bytes in {corpus}")


if __name__ == "__main__":
    main()
