#!/usr/bin/env python3
import base64
import hashlib
import io
from pathlib import Path
import shutil
import tarfile

ROOT = Path(__file__).resolve().parents[1]
PARTS = ROOT / ".openforge-overlay"
EXPECTED_B64_SHA256 = "c3deb11fc54b8e13479fbe8b2fd7b6e9781ab5a7ccb0cc1e334950d8605edd12"
EXPECTED_ARCHIVE_SHA256 = "0ed2fc875ea10e78a6b96d629bb6301a696e79da24175e660f61adac689600ab"

chunks = []
for index in range(11):
    path = PARTS / f"chunk-{index:02d}"
    if not path.is_file():
        raise SystemExit(f"missing overlay chunk: {path}")
    chunks.append(path.read_text(encoding="ascii"))

encoded = "".join(chunks)
encoded_sha = hashlib.sha256(encoded.encode("ascii")).hexdigest()
if encoded_sha != EXPECTED_B64_SHA256:
    raise SystemExit(
        f"overlay base64 checksum mismatch: expected {EXPECTED_B64_SHA256}, got {encoded_sha}"
    )

archive = base64.b64decode(encoded, validate=True)
archive_sha = hashlib.sha256(archive).hexdigest()
if archive_sha != EXPECTED_ARCHIVE_SHA256:
    raise SystemExit(
        f"overlay archive checksum mismatch: expected {EXPECTED_ARCHIVE_SHA256}, got {archive_sha}"
    )

with tarfile.open(fileobj=io.BytesIO(archive), mode="r:xz") as tar:
    members = tar.getmembers()
    root = ROOT.resolve()
    for member in members:
        target = (ROOT / member.name).resolve()
        if not target.is_relative_to(root):
            raise SystemExit(f"unsafe archive path: {member.name}")
        if member.issym() or member.islnk():
            raise SystemExit(f"archive links are not allowed: {member.name}")
    tar.extractall(ROOT, members=members)

# The transport mechanism is deliberately ephemeral.  The production commit
# contains only the OpenForge implementation, tests, docs and configuration.
for transient in [
    ROOT / "scripts" / "apply_ollama_overlay.py",
    ROOT / ".github" / "workflows" / "apply-ollama-overlay.yml",
]:
    if transient.exists():
        transient.unlink()

if PARTS.exists():
    shutil.rmtree(PARTS)
