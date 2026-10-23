"""Compile fixture bytecode against the REAL upstream API, never the shim.

Pass converter arguments after --. The resulting fixture.jar is an ordinary
Bukkit plugin JAR; convert.py takes it exactly as it takes third-party JARs.
"""
import argparse
from pathlib import Path
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--java-home", type=Path, required=True)
    parser.add_argument("--spigot-api", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("converter_args", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent
    args.output.mkdir(parents=True, exist_ok=True)
    classes = args.output / "fixture-classes"
    subprocess.run([str(args.java_home / "bin/javac"), "--release", "17", "-cp", str(args.spigot_api),
        "-d", str(classes), str(root / "fixture/src/example/HelloPlugin.java")], check=True)
    jar = args.output / "fixture.jar"
    subprocess.run([str(args.java_home / "bin/jar"), "--create", "--file", str(jar), "-C", str(classes), ".",
        "-C", str(root / "fixture"), "plugin.yml"], check=True)
    forwarded = args.converter_args
    if forwarded and forwarded[0] == "--":
        forwarded = forwarded[1:]
    subprocess.run([sys.executable, str(root / "convert.py"), "--java-home", str(args.java_home),
        "--jar", str(jar), "--output", str(args.output / "package"), *forwarded], check=True)


if __name__ == "__main__":
    main()
