#!/usr/bin/env python3
"""Download and validate the Pizza&Chili repetitive corpus."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

from download import BASE, sha256

URL = "https://pizzachili.dcc.uchile.cl/repcorpus"
# Every file the server lists, as (category, archive SHA-256, extracted bytes).
# Digests were pinned from the Chilean mirror's .7z archives on 2026-09-27;
# the corpus publishes no checksums of its own.
FILES = {
    "Escherichia_Coli": ("real", "c27d1929f195cb670d0ab4c16bb7f5dfc7709aba8c0816dca28166052baecb81", 112689515),
    "cere": ("real", "b17d3ac9e8de888b426931f9b67a0d246c5d3634c333a3d09c90f27b0185f840", 461286644),
    "coreutils": ("real", "8f8956e02ded27886d7bca063160fb203e146e7e42290c5ea7415361bbdeed10", 205281778),
    "einstein.de.txt": ("real", "b20ca3b46846c27d8be1b47245e5b857098dd787ce568db11ce52d245ab79c97", 92758441),
    "einstein.en.txt": ("real", "9b00289583161b8162367752dad76583b2984a3dba902a12e1a551f4950249a1", 467626544),
    "influenza": ("real", "3b24370bead5cd6ee9c3ccd0ef7a1b0a7c12e6756e6301ad40a532626a837ab3", 154808555),
    "kernel": ("real", "fab9d60b27d2e6056c1d9e6c3af833e601a40fb9aef3bffd7fd08cdcbd133e3b", 257961616),
    "para": ("real", "3b69ac8eeea2af731f672011e2279ef7285253b6a2d2f793291131d51f5acc19", 429265758),
    "world_leaders": ("real", "feee454f2458b15b9287d9914be82294556cb04a9f0d8273bd2dab00bfc6a5c0", 46968181),
    "dblp.xml.00001.1": ("pseudo-real", "48058ec95df75a2ec1367f1ee52ace3abf41a24ffe08962536a53bd3a21ee362", 104857600),
    "dblp.xml.00001.2": ("pseudo-real", "43d8bbdc7fb437e563ef2a3daaa03a985f5bb369fd1783b22ec63c3d6747ef8b", 104857600),
    "dblp.xml.0001.1": ("pseudo-real", "cf00bd822586fee6af429daf23d4db82288be55a9061cba526f4c0d1f045dc6c", 104857600),
    "dblp.xml.0001.2": ("pseudo-real", "48eb61f9045be6d5fdbd49699763ec2435d9a136de6f2915d07fe2473f2e4cbb", 104857600),
    "dna.001.1": ("pseudo-real", "ed86d69e611ceb6633266994090d0ec97e68cf15175508c6203a6a755339b457", 104857600),
    "english.001.2": ("pseudo-real", "a3faeaea6bd434493fe75e73336b7475f4dc4fa6af889687a280533d20428855", 104857600),
    "proteins.001.1": ("pseudo-real", "d9312467b33fe8eee1827798f242e18b1daf322d63b8a849d234e20e3d32e95b", 104857600),
    "sources.001.2": ("pseudo-real", "df84036a5079c86ecbd8ede6dbd43e1db9d3c6bb7142253b63470f02cb776e1a", 104857600),
    "fib41": ("artificial", "58bbb986bd0d715cea992ae13ad06c3ef2ec2e1281944f496f7e63cbd730632f", 267914296),
    "rs.13": ("artificial", "06051e5f20999f314197d3e5639dc4c16f5e20710970145e8fa554d5c44ffe8b", 216747218),
    "tm29": ("artificial", "a1aae78213c5fdb59fcc418fec392b960cbcf980f1738868994db82e78a778c1", 268435456),
    "Android": ("logs", "81892aaf2bbcfb4f2ef08f70c02239526734d8401377df7984bd9fd89a0f5f20", 104857600),
    "BGL": ("logs", "2478d4c8b613298d923ccd40d408052f903464bb39d52412614011d7f8cf6571", 104857600),
    "EDGAR": ("logs", "478bcd544decabcb98ceff15da9f442ec806d752ba88f70709112c28ca1134c9", 104857600),
    "HDFS_1": ("logs", "eb7a5f8616910bb8739dff9da67fa53f575600851ecdb7b636866debc1c25524", 104857600),
    "Horspool": ("logs", "4535cf14c78fd231bcf0c130b14c3f2265a128b7406eee0d803a2b3622a02560", 571839680),
    "NQueens": ("logs", "4c4146c8616dbaf802be1ce3c6d7597e793e4092e25d6c2058df86b352bf1b94", 302590158),
    "Quicksort": ("logs", "70afdce534eea52326caa4930b3e1d3a773190dcbff181a90b398d9fc1ea6aeb", 381494534),
    "Thunderbird": ("logs", "8ea96dc87685025e7d070c3ea145ad870845be907cbcb2c2fc2e11f6d70d1222", 104857600),
    "Windows": ("logs", "178d13f2d9511dcfbb349b411d085a343cc5c39dcbfb2256ffa6d3ef0724ccfd", 104857600),
}
SIZES = {name: size for name, (_, _, size) in FILES.items()}


def fetch(name, archive):
    """Downloads one archive unless a copy with the pinned digest exists."""
    category, digest, _ = FILES[name]
    if archive.exists() and sha256(archive) == digest:
        return
    with tempfile.TemporaryDirectory(prefix="download-", dir=archive.parent) as tmp:
        partial = Path(tmp) / archive.name
        subprocess.run(["curl", "--fail", "--location", "--retry", "3", "--connect-timeout", "30",
                        "--silent", "--show-error", "--output", str(partial),
                        f"{URL}/{category}/{archive.name}"], check=True)
        if sha256(partial) != digest:
            raise ValueError(f"{archive.name} does not match the pinned SHA-256")
        partial.replace(archive)


def extract(sevenzip, name, archive, corpus):
    """Extracts the single member `name`, which 7z checks against its CRC."""
    target = corpus / name
    if target.exists() and target.stat().st_size == SIZES[name]:
        return
    listing = subprocess.run([sevenzip, "l", "-slt", "-ba", str(archive)], check=True,
                             text=True, stdout=subprocess.PIPE).stdout
    paths = [line[len("Path = "):] for line in listing.splitlines() if line.startswith("Path = ")]
    if paths != [name]:
        raise ValueError(f"Unexpected members in {archive.name}: {paths}")
    with tempfile.NamedTemporaryFile(dir=corpus, prefix=f".{name}-", delete=False) as stream:
        temporary = Path(stream.name)
        try:
            subprocess.run([sevenzip, "x", "-so", str(archive), name], stdout=stream, check=True)
        except BaseException:
            temporary.unlink()
            raise
    if temporary.stat().st_size != SIZES[name]:
        temporary.unlink()
        raise ValueError(f"Unexpected size for {name}")
    temporary.replace(target)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", type=Path, default=BASE / "data")
    parser.add_argument("--files", nargs="+", choices=FILES, help="Fetch only these files")
    args = parser.parse_args()
    sevenzip = shutil.which("7z") or shutil.which("7zz")
    if not sevenzip:
        parser.error("7z (p7zip or 7-Zip) is required to extract the archives")
    directory = args.data_dir.resolve()
    archives, corpus = directory / "repcorpus-archives", directory / "repcorpus"
    archives.mkdir(parents=True, exist_ok=True)
    corpus.mkdir(exist_ok=True)
    manifest_path = directory / "repcorpus-manifest.json"
    manifest = {"source": URL, "files": {}}
    if manifest_path.exists():
        manifest = json.loads(manifest_path.read_text())
    for name in args.files or list(FILES):
        category, digest, size = FILES[name]
        archive = archives / f"{name}.7z"
        print(f"{name}: fetching and extracting", flush=True)
        fetch(name, archive)
        extract(sevenzip, name, archive, corpus)
        extracted = sha256(corpus / name)
        recorded = manifest["files"].get(name, {}).get("sha256")
        if recorded not in (None, extracted):
            raise ValueError(f"{name} changed since it was first extracted; delete it and rerun")
        manifest["files"][name] = {"bytes": size, "category": category, "archive_sha256": digest,
                                   "archive_bytes": archive.stat().st_size, "sha256": extracted}
    manifest["files"] = {name: manifest["files"][name] for name in FILES if name in manifest["files"]}
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    count = len(args.files or FILES)
    total = sum(SIZES[name] for name in args.files or FILES)
    print(f"Validated {count} files, {total:,} bytes in {corpus}")


if __name__ == "__main__":
    main()
