"""Explicit offline Bukkit JAR -> Solaris WIT component conversion (requires PyYAML).

This compiler toolchain is not a sandbox: convert trusted input only. JDK 17,
Maven, Binaryen 123, the Rust adapter and plugin_component encoder are inputs.
The blocks report MUST be the deployment's native vanilla cache reports/blocks.json.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import zipfile

import yaml
from prune_exports import prune

ROOT = Path(__file__).resolve().parent
IDENT = re.compile(r"[A-Za-z_$][A-Za-z0-9_$]*(?:\.[A-Za-z_$][A-Za-z0-9_$]*)*\Z")


def run(*args, env=None):
    subprocess.run([str(arg) for arg in args], check=True, env=env)


def metadata(jar):
    with zipfile.ZipFile(jar) as archive:
        data = yaml.safe_load(archive.read("plugin.yml"))
    if not isinstance(data, dict):
        raise ValueError("plugin.yml must be a mapping")
    allowed = {"main", "name", "version", "api-version", "commands", "description", "author", "authors", "website"}
    unsupported = set(data) - allowed
    if unsupported:
        raise ValueError(f"unsupported plugin.yml keys: {sorted(unsupported)}")
    for field in ("main", "name", "version"):
        if not isinstance(data.get(field), str) or not data[field]:
            raise ValueError(f"plugin.yml {field} must be a nonempty string")
    if not IDENT.fullmatch(data["main"]):
        raise ValueError("invalid main class")
    commands = data.get("commands") or {}
    if not isinstance(commands, dict) or len(commands) > 128:
        raise ValueError("commands must be a mapping with at most 128 entries")
    for name, options in commands.items():
        if not isinstance(name, str) or not re.fullmatch(r"[a-z][a-z0-9_-]{0,63}", name):
            raise ValueError("command names must be lowercase native command identifiers")
        options = options or {}
        if not isinstance(options, dict) or set(options) - {"description", "usage"}:
            raise ValueError(f"unsupported command metadata: {name} (aliases/permissions are not translated)")
        if any(not isinstance(value, str) for value in options.values()):
            raise ValueError("command description/usage must be strings")
        commands[name] = options
    data["commands"] = commands
    return data


def material_source(report):
    data = json.loads(report.read_text())
    states = {}
    names = set()
    for key, block in data.items():
        if not re.fullmatch(r"minecraft:[a-z0-9_]+", key):
            raise ValueError(f"unsupported material namespace: {key}")
        name = key.split(":")[1].upper()
        names.add(name)
        for state in block["states"]:
            ident = state["id"]
            if not isinstance(ident, int) or ident < 0 or ident > 2147483647 or ident in states:
                raise ValueError("invalid/duplicate block state id")
            states[ident] = name
    if not {"AIR", "CAVE_AIR", "VOID_AIR"} <= names:
        raise ValueError("blocks report must contain all native air materials")
    ranges = []
    for ident, name in sorted(states.items()):
        if ranges and ranges[-1][2] == name and ranges[-1][1] + 1 == ident:
            ranges[-1] = (ranges[-1][0], ident, name)
        else:
            ranges.append((ident, ident, name))
    lookup = "\n".join(f"if (id >= {first} && id <= {last}) return Material.{name};" for first, last, name in ranges)
    enum = "package org.bukkit;\npublic enum Material {" + ",".join(sorted(names)) + ";\n"
    enum += "public boolean isAir() { return this == AIR || this == CAVE_AIR || this == VOID_AIR; }\n}\n"
    mapping = "package org.solaris.bukkit;\nimport org.bukkit.Material;\npublic final class Materials {\n"
    mapping += "public static Material of(int id) {\n" + lookup
    mapping += '\nthrow new IllegalArgumentException("unknown deployment block state");\n}\n}\n'
    return enum, mapping


def entry_source(meta, handlers):
    priorities = ["LOWEST", "LOW", "NORMAL", "HIGH", "HIGHEST", "MONITOR"]
    dispatch = []
    for priority in priorities:
        dispatch.append("for (org.bukkit.event.Listener listener : Runtime.listeners) {")
        for owner, method, event, order, ignore in handlers:
            if order != priority:
                continue
            if not all(IDENT.fullmatch(value) for value in (owner, method, event)):
                raise ValueError("invalid handler identifier")
            guard = ""
            if ignore == "true":
                guard = " && (!(event instanceof org.bukkit.event.Cancellable) || !((org.bukkit.event.Cancellable) event).isCancelled())"
            dispatch.append(f"if (listener instanceof {owner} && event instanceof {event}{guard}) (({owner}) listener).{method}(({event}) event);")
        dispatch.append("}")
    registrations = "\n".join(f"Runtime.commands.put({json.dumps(name)}, new PluginCommand({json.dumps(name)}, Runtime.plugin));" for name in meta["commands"])
    usage = "\n".join(f"if (name.equals({json.dumps(name)})) player.sendMessage({json.dumps(options['usage'])}.replace(\"<command>\", name));" for name, options in meta["commands"].items() if options.get("usage"))
    return """package org.solaris.bukkit;
import org.teavm.interop.Export;
import org.bukkit.Material;
import org.bukkit.command.PluginCommand;
import org.bukkit.event.Event;
import org.bukkit.event.player.PlayerJoinEvent;
import org.bukkit.event.block.BlockBreakEvent;
public final class EntryPoint {
    public static void main(String[] args) {}
    @Export(name = "enable") public static void enable() {
        Runtime.plugin = new MAIN();
        REGISTRATIONS
        Runtime.plugin.onEnable();
    }
    @Export(name = "disable") public static void disable() { Runtime.plugin.onDisable(); }
    private static void dispatch(Event event) { DISPATCH }
    @Export(name = "deliver") public static void deliver(int kind, long session, int argc) {
        SnapshotPlayer player = new SnapshotPlayer(session, Bridge.input(0), Bridge.input(1));
        if (kind == 1) { dispatch(new PlayerJoinEvent(player)); return; }
        String name = Bridge.input(2);
        PluginCommand command = Runtime.commands.get(name);
        if (command == null) throw new IllegalArgumentException("undeclared command");
        String[] args = new String[argc];
        for (int i = 0; i < argc; i++) args[i] = Bridge.input(i + 3);
        if (!command.execute(player, name, args)) { USAGE }
    }
    @Export(name = "break_block") public static int breakBlock(long session, int oldState, int newState, int x, int y, int z) {
        Material previous = Materials.of(oldState), proposed = Materials.of(newState);
        if (previous.isAir() || !proposed.isAir()) return 0;
        BlockBreakEvent event = new BlockBreakEvent(new SnapshotBlock(previous, x, y, z),
            new SnapshotPlayer(session, Bridge.input(0), null));
        dispatch(event);
        return event.isCancelled() ? 1 : 0;
    }
}
""".replace("MAIN", meta["main"]).replace("REGISTRATIONS", registrations).replace("DISPATCH", "\n".join(dispatch)).replace("USAGE", usage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("java-home", "maven", "binaryen-bin", "jar", "blocks-report", "adapter", "component-encoder", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    meta = metadata(args.jar)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    java, javac = args.java_home / "bin/java", args.java_home / "bin/javac"
    env = dict(os.environ, JAVA_HOME=str(args.java_home), MAVEN_OPTS="-Xmx1g")
    classpath_file = output / "classpath.txt"
    run(args.maven, "-B", "-q", "-f", ROOT / "pom.xml", "compile", "dependency:build-classpath",
        f"-Dmdep.outputFile={classpath_file}", env=env)
    dependencies = classpath_file.read_text().strip()
    compiler_cp = str(ROOT / "target/classes") + ":" + dependencies
    handler_file = output / "handlers.tsv"
    run(java, "-Xmx1g", "-cp", compiler_cp, "org.solaris.bukkit.Inspect", args.jar, handler_file)
    handlers = [line.split("\t") for line in handler_file.read_text().splitlines()]
    linked = output / "linked.jar"
    run(java, "-Xmx1g", "-cp", compiler_cp, "org.solaris.bukkit.RemapLogger", args.jar, linked)
    generated = output / "generated"
    generated.mkdir(exist_ok=True)
    enum, mapping = material_source(args.blocks_report)
    sources = {"EntryPoint.java": entry_source(meta, handlers), "Material.java": enum, "Materials.java": mapping}
    for name, source in sources.items():
        (generated / name).write_text(source)
    shim_classes = output / "shim-classes"
    run(javac, "--release", "17", "-proc:none", "-cp", dependencies + ":" + str(linked), "-d", shim_classes,
        *sorted((ROOT / "shim").rglob("*.java")), *[generated / name for name in sources])
    compiled = output / "compiled"
    run(java, "-Xmx1g", "-cp", compiler_cp, "org.solaris.bukkit.Compile",
        "org.solaris.bukkit.EntryPoint", compiled, shim_classes, linked)
    narrowed = output / "java-pruned.wasm"
    narrowed.write_bytes(prune((compiled / "bukkit.wasm").read_bytes(),
        {"enable", "disable", "deliver", "break_block", "teavm_catchException", "memory"}))
    java_module = output / "java.wasm"
    run(args.binaryen_bin / "wasm-opt", narrowed, "--enable-bulk-memory", "--remove-unused-module-elements", "--dce", "--vacuum", "-o", java_module)
    merged = output / "merged.wasm"
    run(args.binaryen_bin / "wasm-merge", args.adapter, "solaris", java_module, "bukkit", ROOT / "faults.wat", "teavm",
        "--enable-multimemory", "--enable-bulk-memory", "--rename-export-conflicts", "-o", merged)
    run(args.component_encoder, merged, output / "plugin.wasm")
    api = re.search(r"package solaris:plugin@([^;]+);", (ROOT / "../../crates/mc-script/wit/plugin.wit").read_text()).group(1)
    ident = re.sub(r"[^a-z0-9-]", "-", meta["name"].lower()).strip("-")
    if not ident:
        raise ValueError("plugin name does not produce a native package id")
    manifest = f'id = {json.dumps(ident)}\nname = {json.dumps(meta["name"])}\nversion = {json.dumps(meta["version"])}\napi = {json.dumps(api)}\n'
    manifest += "player_commands = " + json.dumps(list(meta["commands"])) + "\n"
    if any(handler[2].endswith("PlayerJoinEvent") for handler in handlers):
        manifest += 'events = ["player.joined"]\n'
    if any(handler[2].endswith("BlockBreakEvent") for handler in handlers):
        manifest += 'hooks = ["before-build"]\n'
    (output / "plugin.toml").write_text(manifest)
    (output / "conversion.json").write_text(json.dumps({"jar_sha256": hashlib.sha256(args.jar.read_bytes()).hexdigest(),
        "blocks_sha256": hashlib.sha256(args.blocks_report.read_bytes()).hexdigest(), "api": api}, indent=2) + "\n")
    print(output / "plugin.wasm")


if __name__ == "__main__":
    main()
