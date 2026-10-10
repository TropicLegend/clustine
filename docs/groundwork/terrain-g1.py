"""Spike G1 of the terrain plan: is the unmodified official server a reference for terrain?

Starts the official server on seed 13579 with a data pack made from its own biome
files with their features taken out, has it load given chunks, stops it, and reads the
region files: which statuses were saved, and the MD5 of the blocks of the chunks left at
the terrain status, as SteelMC's fixture defines it.

Usage: python3 -I terrain-g1.py <run directory> <reports/blocks.json>
           <pack: none|nofeatures> <freeze: 0|1> <chunk x> <chunk z> [<chunk x> <chunk z> ...]

The jar is the one `cargo datagen` downloads (or `SERVER_JAR`), and `blocks.json` is
among the reports of its data generator. Running this agrees to the Minecraft EULA for
the run. Nothing is written outside the run directory. It is a trial, kept so that its
result can be made again; the tool the plan builds from it is step G7.
"""

import hashlib
import io
import json
import os
import socket
import struct
import subprocess
import sys
import time
import zipfile
import zlib

JAR = os.path.abspath(os.environ.get("SERVER_JAR", "target/datagen/26.3/server.jar"))
JAVA = os.environ.get("JAVA", "java")
SEED = 13579


def inner_jar():
    outer = zipfile.ZipFile(JAR)
    name = [n for n in outer.namelist() if n.startswith("META-INF/versions/") and n.endswith(".jar")][0]
    return zipfile.ZipFile(io.BytesIO(outer.read(name)))


def write_pack(world, kind):
    """A data pack in the world's `datapacks`: the jar's biomes without their features."""
    if kind == "none":
        return
    root = os.path.join(world, "datapacks", "spike")
    biomes = os.path.join(root, "data", "minecraft", "worldgen", "biome")
    os.makedirs(biomes, exist_ok=True)
    meta = {"pack": {"description": "spike", "min_format": 121, "max_format": 121}}
    with open(os.path.join(root, "pack.mcmeta"), "w") as out:
        json.dump(meta, out)
    jar = inner_jar()
    count = 0
    for name in jar.namelist():
        if name.startswith("data/minecraft/worldgen/biome/") and name.endswith(".json"):
            biome = json.loads(jar.read(name))
            biome["features"] = []
            with open(os.path.join(biomes, os.path.basename(name)), "w") as out:
                json.dump(biome, out)
            count += 1
    print(f"pack: {count} biomes without features")


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def run_server(directory, pack, freeze, chunks):
    os.makedirs(directory, exist_ok=True)
    world = os.path.join(directory, "world")
    write_pack(world, pack)
    with open(os.path.join(directory, "eula.txt"), "w") as out:
        out.write("eula=true\n")
    properties = {
        "level-seed": SEED,
        "level-name": "world",
        "server-ip": "127.0.0.1",
        "server-port": free_port(),
        "online-mode": "false",
        "view-distance": 2,
        "simulation-distance": 2,
        "max-tick-time": -1,
        "sync-chunk-writes": "true",
        "enable-rcon": "false",
        "enable-query": "false",
    }
    with open(os.path.join(directory, "server.properties"), "w") as out:
        for key, value in properties.items():
            out.write(f"{key}={value}\n")
    log = open(os.path.join(directory, "server.log"), "w")
    server = subprocess.Popen(
        [JAVA, "-Xmx2G", "-jar", JAR, "nogui"],
        cwd=directory,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )

    def wait_for(what, patience):
        end = time.time() + patience
        while time.time() < end:
            line = server.stdout.readline()
            if not line:
                raise SystemExit(f"the server ended while waiting for {what!r}; see server.log")
            log.write(line)
            log.flush()
            if what in line:
                return line
        raise SystemExit(f"no {what!r} within {patience} s; see server.log")

    def say(command):
        server.stdin.write(command + "\n")
        server.stdin.flush()

    wait_for("Done (", 300)
    if freeze:
        say("tick freeze")
    for x, z in chunks:
        say(f"forceload add {x * 16} {z * 16}")
        wait_for("force loaded", 120)
    # Generation is under way on other threads; the save is asked for when the
    # chunk is there, which a query of the last one says.
    last = chunks[-1]
    for _ in range(120):
        time.sleep(1)
        say(f"execute if loaded {last[0] * 16} 0 {last[1] * 16} run say there")
        try:
            wait_for("there", 2)
            break
        except SystemExit:
            continue
    time.sleep(3)
    say("save-all flush")
    wait_for("Saved the game", 300)
    say("stop")
    server.wait(timeout=300)
    log.write(server.stdout.read())
    log.close()
    return world


# --- NBT, as the region files have it -------------------------------------------------


def read_nbt(data):
    stream = io.BytesIO(data)

    def take(fmt):
        size = struct.calcsize(fmt)
        return struct.unpack(fmt, stream.read(size))[0]

    def string():
        return stream.read(take(">H")).decode("utf-8", "replace")

    def payload(kind):
        if kind == 1:
            return take(">b")
        if kind == 2:
            return take(">h")
        if kind == 3:
            return take(">i")
        if kind == 4:
            return take(">q")
        if kind == 5:
            return take(">f")
        if kind == 6:
            return take(">d")
        if kind == 7:
            return stream.read(take(">i"))
        if kind == 8:
            return string()
        if kind == 9:
            of = take(">b")
            return [payload(of) for _ in range(take(">i"))]
        if kind == 10:
            compound = {}
            while True:
                tag = take(">b")
                if tag == 0:
                    return compound
                name = string()
                compound[name] = payload(tag)
        if kind == 11:
            return [take(">i") for _ in range(take(">i"))]
        if kind == 12:
            count = take(">i")
            return list(struct.unpack(f">{count}q", stream.read(8 * count)))
        raise ValueError(f"tag {kind}")

    kind = take(">b")
    string()
    return payload(kind)


def region_directory(world):
    """Where 26.3 keeps the overworld's region files."""
    return os.path.join(world, "dimensions", "minecraft", "overworld", "region")


def chunks_of(world):
    """Every chunk in the overworld's region files, by position."""
    found = {}
    regions = region_directory(world)
    for name in sorted(os.listdir(regions)):
        if not name.endswith(".mca"):
            continue
        with open(os.path.join(regions, name), "rb") as file:
            data = file.read()
        if len(data) < 8192:
            continue
        for index in range(1024):
            entry = struct.unpack(">I", data[index * 4 : index * 4 + 4])[0]
            offset, sectors = entry >> 8, entry & 0xFF
            if offset == 0 or sectors == 0:
                continue
            start = offset * 4096
            length = struct.unpack(">I", data[start : start + 4])[0]
            compression = data[start + 4]
            body = data[start + 5 : start + 4 + length]
            if compression == 2:
                body = zlib.decompress(body)
            elif compression == 1:
                body = zlib.decompress(body, 31)
            elif compression != 3:
                raise SystemExit(f"compression {compression} in {name}")
            chunk = read_nbt(body)
            found[(chunk["xPos"], chunk["zPos"])] = chunk
    return found


def state_ids(report):
    with open(report) as file:
        blocks = json.load(file)
    ids, air = {}, set()
    for name, block in blocks.items():
        for state in block["states"]:
            key = (name, tuple(sorted(state.get("properties", {}).items())))
            ids[key] = state["id"]
            if state.get("default"):
                # 26.3 writes a block's default state as its name alone.
                ids[name] = state["id"]
            if name in ("minecraft:air", "minecraft:cave_air", "minecraft:void_air"):
                air.add(state["id"])
    return ids, air


def block_hash(chunk, ids, air):
    """SteelMC's hash of a chunk's blocks: sections bottom to top, y, z, x."""
    sections = {section["Y"]: section for section in chunk["sections"]}
    lowest = chunk["yPos"]
    md5 = hashlib.md5()
    for y in range(lowest, lowest + 24):
        states = sections.get(y, {}).get("block_states")
        if states is None:
            md5.update(b"\0")
            continue
        palette = []
        for entry in states["palette"]:
            # A list of several kinds of entries wraps each in a compound.
            if isinstance(entry, dict) and list(entry) == [""]:
                entry = entry[""]
            if isinstance(entry, str):
                palette.append(ids[entry])
            else:
                name = entry.get("id", entry.get("Name"))
                properties = entry.get("properties", entry.get("Properties", {}))
                palette.append(ids[(name, tuple(sorted(properties.items())))])
        data = states.get("data")
        if data is None:
            section = [palette[0]] * 4096
        else:
            bits = max(4, (len(palette) - 1).bit_length())
            per_long = 64 // bits
            mask = (1 << bits) - 1
            section = []
            for index in range(4096):
                word = data[index // per_long] & 0xFFFFFFFFFFFFFFFF
                section.append(palette[(word >> ((index % per_long) * bits)) & mask])
        if all(state in air for state in section):
            md5.update(b"\0")
        else:
            md5.update(struct.pack(">4096i", *section))
    return md5.hexdigest()


def main():
    directory, report, pack, freeze = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4] == "1"
    numbers = [int(value) for value in sys.argv[5:]]
    chunks = list(zip(numbers[0::2], numbers[1::2]))
    world = os.path.join(directory, "world")
    if not os.path.exists(region_directory(world)):
        world = run_server(directory, pack, freeze, chunks)
    ids, air = state_ids(report)
    found = chunks_of(world)
    by_status = {}
    for chunk in found.values():
        by_status[chunk.get("Status")] = by_status.get(chunk.get("Status"), 0) + 1
    print("statuses saved:", by_status)
    out = {}
    for (x, z), chunk in sorted(found.items()):
        out[f"{x},{z}"] = {"status": chunk.get("Status"), "blocks": block_hash(chunk, ids, air)}
    with open(os.path.join(directory, "hashes.json"), "w") as file:
        json.dump(out, file, indent=1)
    for x, z in chunks:
        print(x, z, out.get(f"{x},{z}"))


main()
