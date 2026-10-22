"""Pinned MC 26.1.2 + DH 3.3.3 Java graphical gate (protocol 16).

The sibling Loader worktree owns the Java scenario and client driver.
This profile verifies publisher pins and retains canonical command receipts.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
from pathlib import Path
import sys
from typing import Any

from ..profiles import ProfileContext

ARTIFACTS = json.loads((Path(__file__).parents[1] / "dh-artifacts.json").read_text())
BASELINE = next(item for item in ARTIFACTS["artifacts"] if item["minecraft"] == "26.1.2")
MC_VERSION = BASELINE["minecraft"]
DH_VERSION = BASELINE["distant_horizons"]
DH_PROTOCOL = ARTIFACTS["protocol"]
DH_FILENAME = BASELINE["filename"]
DH_URL = BASELINE["url"]
DH_SHA512 = BASELINE["sha512"]
DH_SHA1 = BASELINE["sha1"]
DH_SIZE = BASELINE["size"]
PLATFORMS = ("fabric", "neoforge")
DEFAULT_JAR_REL = Path(".analysis/DHCompat/artifacts") / DH_FILENAME


def _flag_value(args: list[str], flag: str) -> str | None:
    for index, token in enumerate(args):
        if token == flag and index + 1 < len(args):
            return args[index + 1]
        if token.startswith(flag + "="):
            return token[len(flag) + 1:]
    return None


def _loader_root() -> Path:
    override = os.environ.get("SOLARIS_LOADER_ROOT")
    if override:
        return Path(override).expanduser()
    from .. import runtime

    return runtime.REPO_ROOT.parent / "solaris-loader"


def _check_server_candidate() -> dict[str, Any]:
    override = os.environ.get("SOLARIS_SERVER_BINARY")
    if not override:
        return {
            "present": False,
            "detail": "SOLARIS_SERVER_BINARY is unset; build the worktree "
            "solaris-v2 and export its absolute path (no implicit build here).",
        }
    binary = Path(override).expanduser()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        return {"present": False, "detail": f"server candidate is not executable: {binary}"}
    return {
        "present": True,
        "path": str(binary.resolve()),
        "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "note": "Executable fingerprint only; CLI help cannot prove DH support.",
    }


def collect_prerequisites(platform: str) -> dict[str, Any]:
    from .. import runtime

    repo_root = runtime.REPO_ROOT
    jar_env = os.environ.get("SOLARIS_DH_JAR")
    jar_path = Path(jar_env).expanduser() if jar_env else repo_root / DEFAULT_JAR_REL
    jar: dict[str, Any] = {
        "mc": MC_VERSION,
        "dh": DH_VERSION,
        "protocol": DH_PROTOCOL,
        "url": DH_URL,
        "sha512": DH_SHA512,
        "sha1": DH_SHA1,
        "size": DH_SIZE,
        "local_path": str(jar_path),
        "present": False,
    }
    if jar_path.is_file():
        actual_size = jar_path.stat().st_size
        jar["present"] = True
        jar["size_ok"] = actual_size == DH_SIZE
        content = jar_path.read_bytes()
        jar["sha512_ok"] = hashlib.sha512(content).hexdigest() == DH_SHA512
        jar["sha1_ok"] = hashlib.sha1(content).hexdigest() == DH_SHA1
        if actual_size != DH_SIZE:
            jar["detail"] = f"size {actual_size} != published {DH_SIZE}; re-fetch, do not trust"
    else:
        jar["detail"] = (
            f"not downloaded; fetch {DH_URL} and verify sha512/sha1 before use. "
            "No download happens inside this gate."
        )

    loader_root = _loader_root()
    loader: dict[str, Any] = {"path": str(loader_root), "present": loader_root.is_dir()}
    if loader_root.is_dir():
        props = loader_root / "gradle.properties"
        if props.is_file():
            text = props.read_text(errors="replace")
            loader["minecraftVersion_26_1_2"] = "minecraftVersion=26.1.2" in text
        driver = loader_root / "tools/dh_scenario.py"
        loader["dh_scenario_found"] = driver.is_file()
        loader["dh_scenario_hits"] = [str(driver)] if driver.is_file() else []
        if not loader["dh_scenario_found"]:
            loader["detail"] = (
                "sibling loader checkout present but no DH Java scenario entrypoint "
                "found there; the Java driver belongs to solaris-loader, not here."
            )
    else:
        loader["detail"] = "sibling solaris-loader checkout missing"

    server = _check_server_candidate()
    java = shutil.which("java")
    xvfb = shutil.which("Xvfb")
    env = {
        "java25": bool(java),
        "xvfb": bool(xvfb),
        "client_token_set": bool(os.environ.get("SOLARIS_CLIENT_MCP_TOKEN")),
        "client_token_source": "fresh per-client token minted locally by the canonical driver",
    }
    return {
        "baseline": {"mc": MC_VERSION, "dh": DH_VERSION, "protocol": DH_PROTOCOL, "platform": platform},
        "deferred": ["MC 26.3 + DH 3.3.4 (profile merge; Main owns integration)"],
        "dh_jar": jar,
        "loader": loader,
        "server": server,
        "environment": env,
    }


def run(ctx: ProfileContext) -> dict[str, Any]:
    from .. import runtime

    platform = ctx.platform or _flag_value(ctx.extra_args, "--platform")
    if platform not in PLATFORMS:
        raise RuntimeError(
            f"dh-gate requires --platform {'|'.join(PLATFORMS)} (baseline MC {MC_VERSION} + DH {DH_VERSION}), "
            f"got {platform!r}"
        )
    if ctx.client != "java":
        raise ValueError("dh-gate requires the actual Java client, not the native client")
    prereqs = collect_prerequisites(platform)
    (ctx.artifact_dir / "dh-prerequisites.json").write_text(
        json.dumps(prereqs, indent=2, sort_keys=True) + "\n"
    )

    missing: list[str] = []
    jar = prereqs["dh_jar"]
    if not jar["present"]:
        missing.append(f"DH jar missing at {jar['local_path']} (fetch {DH_URL})")
    elif not all(jar.get(key, False) for key in ("size_ok", "sha512_ok", "sha1_ok")):
        missing.append(f"DH jar publisher size/hash mismatch at {jar['local_path']}")
    loader = prereqs["loader"]
    if not loader["present"]:
        missing.append(f"sibling loader checkout missing at {loader['path']}")
    elif not loader.get("dh_scenario_found"):
        missing.append(
            "DH Java scenario missing in sibling solaris-loader "
            f"(checked {loader['path']}/tools/*dh*; sibling owns the driver)"
        )
    server = prereqs["server"]
    if not server.get("present"):
        missing.append(f"server candidate: {server.get('detail')}")
    env = prereqs["environment"]
    if not env["java25"]:
        missing.append("java not on PATH (need Java 25 for the real client)")
    if not env["xvfb"]:
        missing.append("Xvfb not on PATH (need Xvfb for the graphical gate)")
    if not loader.get("minecraftVersion_26_1_2", True):
        missing.append("sibling loader gradle.properties does not pin minecraftVersion=26.1.2")

    if missing:
        raise RuntimeError(
            f"dh-gate baseline MC {MC_VERSION} + DH {DH_VERSION} [{platform}] blocked: "
            + "; ".join(missing)
        )
    from ..profiles import _run_command

    command = [
        sys.executable, str(Path(loader["path"]) / "tools/dh_scenario.py"),
        "--platform", platform,
        "--repo-root", str(runtime.REPO_ROOT),
        "--artifact-dir", str(ctx.artifact_dir),
        "--server-bin", server["path"],
        "--dh-jar", jar["local_path"],
    ]
    _run_command(command, ctx, "dh-java-driver.log")
    result_path = ctx.artifact_dir / "dh-scenario.json"
    result = json.loads(result_path.read_text())
    if result.get("passed") is not True:
        raise RuntimeError(f"DH Java scenario did not pass: {result.get('error', result_path)}")
    return {"scenario": result, "packet_evidence": str(ctx.artifact_dir / "dh-packet-evidence.json")}
