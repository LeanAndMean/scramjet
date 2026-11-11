#!/usr/bin/env python3
"""Makes npm's packages from the release archives in dist, as esbuild's are laid out:

    scripts/npm-packages.py [--pack]

writes dist/npm/scramjet (`@leanandmean/scramjet`: npm/scramjet.js as its `scramjet`, and an
optional dependency on each platform's package) and dist/npm/scramjet-OS-CPU (the program of
dist/scramjet-VERSION-PLATFORM.tar.gz, installed by npm only where its `os` and `cpu` match).
With --pack, `npm pack` makes each one's tarball in dist/npm. Publishing them is not done here.
"""

import json
import re
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCOPE = "@leanandmean"
REPOSITORY = {"type": "git", "url": "git+https://github.com/LeanAndMean/scramjet.git"}
# Rust's ARCH-OS, as scripts/package.sh names the archives, to Node's process.platform and arch.
PLATFORMS = {
    "x86_64-linux": ("linux", "x64"),
    "aarch64-linux": ("linux", "arm64"),
    "aarch64-macos": ("darwin", "arm64"),
    "x86_64-macos": ("darwin", "x64"),
    "x86_64-windows": ("win32", "x64"),
}


def version():
    text = (ROOT / "Cargo.toml").read_text()
    return re.search(r'^version = "(.*)"$', text, re.MULTILINE).group(1)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def common(name, description):
    return {
        "name": name,
        "version": version(),
        "description": description,
        "license": "Apache-2.0",
        "repository": REPOSITORY,
        "homepage": "https://github.com/LeanAndMean/scramjet#readme",
    }


def platform_package(archive, platform, out):
    node_os, cpu = PLATFORMS[platform]
    program = "scramjet.exe" if node_os == "win32" else "scramjet"
    name = f"{SCOPE}/scramjet-{node_os}-{cpu}"
    directory = out / f"scramjet-{node_os}-{cpu}"
    (directory / "bin").mkdir(parents=True)
    with tarfile.open(archive) as tar:
        member = tar.getmember(program)
        with tar.extractfile(member) as source, open(directory / "bin" / program, "wb") as target:
            shutil.copyfileobj(source, target)
    (directory / "bin" / program).chmod(0o755)
    manifest = common(name, f"The scramjet program for {node_os} on {cpu}")
    manifest.update({"os": [node_os], "cpu": [cpu], "files": ["bin/"]})
    if node_os == "linux":
        manifest["libc"] = ["glibc"]
    write_json(directory / "package.json", manifest)
    shutil.copy(ROOT / "LICENSE", directory / "LICENSE")
    (directory / "README.md").write_text(
        f"# {name}\n\nThe `scramjet` program for {node_os} on {cpu}. Install "
        f"[`{SCOPE}/scramjet`](https://www.npmjs.com/package/{SCOPE}/scramjet), which "
        "depends on it, rather than this package.\n"
    )
    return name, directory


def main_package(names, out):
    directory = out / "scramjet"
    (directory / "bin").mkdir(parents=True)
    shutil.copy(ROOT / "npm" / "scramjet.js", directory / "bin" / "scramjet.js")
    (directory / "bin" / "scramjet.js").chmod(0o755)
    manifest = common(f"{SCOPE}/scramjet", "A high-velocity harness for agentic development")
    manifest.update(
        {
            "bin": {"scramjet": "bin/scramjet.js"},
            "files": ["bin/"],
            "engines": {"node": ">=16"},
            "optionalDependencies": {name: version() for name in sorted(names)},
        }
    )
    write_json(directory / "package.json", manifest)
    shutil.copy(ROOT / "LICENSE", directory / "LICENSE")
    shutil.copy(ROOT / "README.md", directory / "README.md")
    return directory


def main():
    pack = sys.argv[1:] == ["--pack"]
    if sys.argv[1:] and not pack:
        sys.exit(f"usage: {sys.argv[0]} [--pack]")
    dist = ROOT / "dist"
    out = dist / "npm"
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True)
    names, directories = [], []
    for platform in PLATFORMS:
        archive = dist / f"scramjet-{version()}-{platform}.tar.gz"
        if archive.exists():
            name, directory = platform_package(archive, platform, out)
            names.append(name)
            directories.append(directory)
    if not names:
        sys.exit(f"no archives of {version()} in {dist}: run scripts/package.sh first")
    directories.append(main_package(names, out))
    for directory in directories:
        print(directory.relative_to(ROOT))
        if pack:
            subprocess.run(
                ["npm", "pack", "--silent", "--pack-destination", str(out)],
                cwd=directory,
                check=True,
            )


if __name__ == "__main__":
    main()
