#!/usr/bin/env python3
"""Package an existing arm64 executable; no Developer ID signing or notarization."""
import argparse
import hashlib
import plistlib
import shutil
import subprocess
import tomllib
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--binary", type=Path, default=Path("target/aarch64-apple-darwin/release/agent-companion"))
parser.add_argument("--output", type=Path, default=Path("dist"))
args = parser.parse_args()
root = Path(__file__).resolve().parent.parent
version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
arches = subprocess.check_output(["lipo", "-archs", str(args.binary)], text=True).strip()
if arches != "arm64":
    parser.error(f"expected Apple Silicon arm64 binary, found: {arches}")
args.output.mkdir(parents=True, exist_ok=True)
app = args.output / "Agent Companion.app"
if app.exists():
    parser.error(f"output already exists: {app}; use a new output directory")
contents = app / "Contents"
(contents / "MacOS").mkdir(parents=True)
(contents / "Resources").mkdir()
shutil.copy2(args.binary, contents / "MacOS" / "agent-companion")
(contents / "MacOS" / "agent-companion").chmod(0o755)
with (contents / "Info.plist").open("wb") as stream:
    plistlib.dump({
        "CFBundleExecutable": "agent-companion",
        "CFBundleIdentifier": "com.wxgopher.agent-companion",
        "CFBundleName": "Agent Companion",
        "CFBundleDisplayName": "Agent Companion",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "LSMinimumSystemVersion": "11.0",
        "LSArchitecturePriority": ["arm64"],
        "NSHighResolutionCapable": True,
        "NSPrincipalClass": "NSApplication",
        "LSApplicationCategoryType": "public.app-category.developer-tools",
    }, stream)
for name in ("README.md", "LICENSE"):
    shutil.copy2(root / name, contents / "Resources" / name)
archive = args.output / f"agent-companion-v{version}-macos-arm64.zip"
subprocess.run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(app), str(archive)], check=True)
digest = hashlib.sha256(archive.read_bytes()).hexdigest()
(args.output / "SHA256SUMS-macos-arm64.txt").write_text(f"{digest}  {archive.name}\n")
print(archive)
