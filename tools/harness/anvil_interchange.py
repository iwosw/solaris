"""Headless 26.1.2 command-fixture -> native -> Anvil semantic-diff gate.

Run only inside an approved mem-run acceptance window. Requires a built candidate
and an already accepted EULA; never accepts terms, deletes worlds or builds code.
The original Java world is retained. A separate native gameplay/export/Java-save
leg proves the CLI destination can be opened and reimported, not gameplay parity.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import threading

from tools.harness import runtime
from tools.harness.quirk_parity import anvil
from tools.harness.quirk_parity.vanilla import VanillaOracle
from tools.harness.vanilla_world_roundtrip import seed_startup_tickets


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def palette_cells(container, count, minimum_bits):
    palette = container['palette']
    if len(palette) == 1:
        return [palette[0]] * count
    bits = max(minimum_bits, (len(palette) - 1).bit_length())
    per_word = 64 // bits
    return [palette[(container['data'][i // per_word] >> ((i % per_word) * bits)) & ((1 << bits) - 1)] for i in range(count)]


def normalize(value, path=''):
    if isinstance(value, dict):
        out = {}
        for key, child in value.items():
            if key.startswith(('Solaris', 'solaris:')) or key == 'LastUpdate':
                continue
            if key in ('block_states', 'biomes'):
                count, minimum = (4096, 4) if key == 'block_states' else (64, 1)
                out[key] = digest(palette_cells(child, count, minimum))
            else:
                out[key] = normalize(child, f'{path}/{key}')
        return out
    if isinstance(value, list):
        out = [normalize(child, path) for child in value]
        sort_key = {'sections': 'Y', 'block_entities': 'x', 'Items': 'Slot'}.get(path.rsplit('/', 1)[-1])
        if sort_key and all(isinstance(v, dict) and sort_key in v for v in out):
            out.sort(key=lambda v: (v[sort_key], v.get('y', 0), v.get('z', 0)))
        return out
    return value


def world_records(world):
    records = {}
    for path in sorted(world.rglob('*')):
        if not path.is_file():
            continue
        relative = path.relative_to(world).as_posix()
        if path.suffix == '.mca' and path.parent.name in ('region', 'entities'):
            for x, z, nbt in anvil.read_region(path):
                # Empty entity regions have no entities to compare.
                if path.parent.name == 'entities':
                    for entity in nbt.get('Entities', []):
                        records[f'{path.parent.relative_to(world)}/UUID:{entity["UUID"]}'] = normalize(entity)
                else:
                    records[f'{relative}#{x},{z}'] = normalize(nbt)
        elif path.suffix == '.dat':
            raw = path.read_bytes()
            records[relative] = normalize(anvil.parse_nbt(gzip.decompress(raw) if raw[:2] == b'\x1f\x8b' else raw))
        elif path.suffix == '.json':
            records[relative] = json.loads(path.read_text())
        elif path.name not in ('session.lock', 'level.dat_old'):
            records[relative] = {'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
    return records


def differences(before, after, path='', additions=None):
    """Source-field equality; separately enumerate every added codec default."""
    if isinstance(before, dict) and isinstance(after, dict):
        result = []
        for key in before:
            if key not in after:
                result.append({'path': f'{path}/{key}', 'before': before[key], 'after': '<missing>'})
            else:
                result.extend(differences(before[key], after[key], f'{path}/{key}', additions))
        if additions is not None:
            additions.extend(f'{path}/{key}' for key in after.keys() - before.keys())
        return result
    if isinstance(before, list) and isinstance(after, list) and len(before) == len(after):
        return [diff for i, (a, b) in enumerate(zip(before, after)) for diff in differences(a, b, f'{path}/{i}', additions)]
    if before != after:
        def brief(value):
            return value if len(str(value)) < 500 else {'sha256': digest(value), 'length': len(value)}
        return [{'path': path, 'before': brief(before), 'after': brief(after)}]
    return []


def commands():
    result = ['tick freeze', 'gamerule minecraft:advance_time false', 'gamerule minecraft:advance_weather false',
              'fill 0 100 0 15 100 15 minecraft:stone', 'fill 0 101 0 15 108 15 minecraft:air',
              'setblock 1 101 1 minecraft:oak_stairs[facing=west,half=top,shape=straight,waterlogged=true]',
              'setblock 2 101 1 minecraft:oak_slab[type=top,waterlogged=true]',
              'fillbiome 0 100 0 3 103 3 minecraft:lush_caves',
              'fillbiome 0 104 0 3 107 3 minecraft:desert']
    blocks = [
        ('chest', '{CustomName:{text:"Named chest"},Items:[{Slot:0b,id:"minecraft:diamond",count:7}]}'),
        ('barrel', '{CustomName:{text:"Named barrel"},Items:[{Slot:2b,id:"minecraft:emerald",count:3}]}'),
        ('furnace', '{CustomName:{text:"Named furnace"},Items:[{Slot:1b,id:"minecraft:coal",count:4}]}'),
        ('hopper[enabled=false]', '{CustomName:{text:"Named hopper"},Items:[{Slot:0b,id:"minecraft:iron_ingot",count:5}]}'),
        ('brewing_stand', '{CustomName:{text:"Named brewer"},Fuel:7b}'),
        ('oak_sign', '{front_text:{messages:[{text:"Front"},{text:""},{text:""},{text:""}],color:"red",has_glowing_text:1b},back_text:{messages:[{text:"Back"},{text:""},{text:""},{text:""}],color:"blue",has_glowing_text:0b}}'),
        ('white_banner', '{patterns:[{pattern:"minecraft:stripe_bottom",color:"red"}]}'),
        ('spawner', '{Delay:123s,SpawnData:{entity:{id:"minecraft:pig"}}}'),
        ('purple_shulker_box', '{Items:[{Slot:4b,id:"minecraft:diamond",count:9}]}'),
        ('lectern[has_book=true]', '{Book:{id:"minecraft:written_book",count:1,components:{"minecraft:written_book_content":{title:{raw:"Fixture"},author:"Oracle",pages:[{raw:{text:"Page one"}}]}}},Page:0}'),
        ('beehive[honey_level=3]', '{bees:[{entity_data:{id:"minecraft:bee"},ticks_in_hive:10,min_ticks_in_hive:600}]}'),
        ('campfire[lit=false]', '{Items:[{Slot:0b,id:"minecraft:beef",count:1}],CookingTimes:[I;10,0,0,0],CookingTotalTimes:[I;600,0,0,0]}'),
        ('jukebox[has_record=true]', '{RecordItem:{id:"minecraft:music_disc_13",count:1}}'),
    ]
    for x, (block, nbt) in enumerate(blocks, 1):
        result.append(f'setblock {x} 101 4 minecraft:{block}{nbt}')
    entities = [
        ('villager', '{NoAI:1b,PersistenceRequired:1b,VillagerData:{type:"minecraft:plains",profession:"minecraft:farmer",level:2},Offers:{Recipes:[{buy:{id:"minecraft:wheat",count:20},sell:{id:"minecraft:emerald",count:1},uses:2,maxUses:16,xp:2}]},Gossips:[{Target:[I;0,0,0,42],Type:"minor_positive",Value:7}]}'),
        ('armor_stand', '{NoGravity:1b,ShowArms:1b,Pose:{Head:[15f,30f,0f]}}'),
        ('item_frame', '{Facing:1b,Fixed:1b,Item:{id:"minecraft:diamond",count:1},ItemRotation:3b}'),
        ('painting', '{Facing:2b,variant:"minecraft:kebab"}'),
        ('chest_minecart', '{Items:[{Slot:0b,id:"minecraft:gold_ingot",count:8}]}'),
        ('furnace_minecart', '{Fuel:200s,PushX:0.25d,PushZ:0.0d}'),
        ('oak_boat', '{}'),
        ('oak_chest_boat', '{Items:[{Slot:0b,id:"minecraft:iron_ingot",count:6}]}'),
    ]
    for x, (entity, nbt) in enumerate(entities, 1):
        result.append(f'summon minecraft:{entity} {x}.5 102 10.5 {nbt}')
    result += ['scoreboard objectives add interchange dummy', 'scoreboard players set Fixture interchange 37',
               'team add fixture', 'team join fixture Fixture', 'team modify fixture color gold',
               'worldborder center 12.5 -4.5', 'worldborder set 512', 'time set 6000']
    return result


def validate_fixture(records):
    block_entities = {
        entity['x']: entity
        for key, chunk in records.items()
        if '/region/' in key and isinstance(chunk, dict)
        for entity in chunk.get('block_entities', [])
        if entity.get('y') == 101 and entity.get('z') == 4 and 1 <= entity.get('x', 0) <= 13
    }
    required = ['chest', 'barrel', 'furnace', 'hopper', 'brewing_stand', 'sign',
                'banner', 'mob_spawner', 'shulker_box', 'lectern', 'beehive', 'campfire', 'jukebox']
    for x, name in enumerate(required, 1):
        if block_entities.get(x, {}).get('id') != 'minecraft:' + name:
            raise RuntimeError(f'Java fixture did not retain block entity {name} at {x},101,4')
    for x in range(1, 6):
        if 'CustomName' not in block_entities[x]:
            raise RuntimeError(f'Java fixture did not retain container name at {x},101,4')
    sign = block_entities[6]
    if 'Front' not in str(sign.get('front_text')) or 'Back' not in str(sign.get('back_text')):
        raise RuntimeError('Java fixture did not retain both sign sides')
    entities = [entity for key, entity in records.items() if '/UUID:' in key]
    required_entities = ['villager', 'armor_stand', 'item_frame', 'painting',
                         'chest_minecart', 'furnace_minecart', 'oak_boat', 'oak_chest_boat']
    for name in required_entities:
        if not any(entity.get('id') == 'minecraft:' + name for entity in entities):
            raise RuntimeError(f'Java fixture did not retain entity {name}')
    villager = next(entity for entity in entities if entity.get('id') == 'minecraft:villager' and entity.get('Offers'))
    if not villager.get('Gossips'):
        raise RuntimeError('Java fixture did not retain villager gossip')
    return {'block_entities': required, 'entities': required_entities, 'both_sign_sides': True,
            'villager_offers_and_gossip': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--jar', type=Path, required=True)
    parser.add_argument('--eula-file', type=Path, required=True)
    parser.add_argument('--cache', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    receipt = {'passed': False, 'scope': 'headless interchange, not client/gameplay parity',
               'commands': [], 'checks': {}, 'normalizations': ['compound order and numeric NBT widths', 'palette encoding/order', 'LastUpdate and Solaris bookkeeping', 'added codec defaults enumerated separately']}
    processes = []
    try:
        if 'eula=true' not in args.eula_file.read_text().splitlines():
            raise RuntimeError('already accepted eula.txt required')
        receipt['binary_sha256'] = hashlib.sha256(args.binary.read_bytes()).hexdigest()
        receipt['jar_sha256'] = hashlib.sha256(args.jar.read_bytes()).hexdigest()

        def launch(home, java=True, config=None):
            home.mkdir(exist_ok=True)
            port = runtime.reserve_port()
            if java:
                shutil.copyfile(args.eula_file, home / 'eula.txt')
                (home / 'server.properties').write_text(f'server-ip=127.0.0.1\nserver-port={port}\nonline-mode=false\nlevel-name=world\nlevel-seed=42\nview-distance=2\nsimulation-distance=2\nmax-players=1\nspawn-protection=0\npause-when-empty-seconds=-1\n')
                cmd = ['java', '-Xms256M', '-Xmx2G', '-jar', str(args.jar), 'nogui']
            else:
                cmd = [str(args.binary), '--config', str(config)]
            oracle = VanillaOracle(home, seed=42, jar=args.jar, use_mem_run=False, timeout=180)
            oracle.log_path = home / ('vanilla.log' if java else 'solaris.log')
            receipt['commands'].append(cmd)
            oracle.process = subprocess.Popen(cmd, cwd=home, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1, start_new_session=True)
            processes.append(oracle.process)
            def collect():
                with oracle.log_path.open('w') as log:
                    for line in oracle.process.stdout:
                        log.write(line); log.flush(); oracle.lines.put(line.rstrip())
                oracle.lines.put(None)
            threading.Thread(target=collect, daemon=True).start()
            if java:
                oracle.expect('Done (', timeout=300)
            else:
                runtime.wait_port(native_port, 300, oracle.process, log_path=oracle.log_path)
            return oracle

        source_home = args.output / 'java-source'
        source_home.mkdir()
        seed_startup_tickets(source_home / 'world')
        source = launch(source_home)
        feedback = []
        for command in commands():
            lines = source.run(command)
            feedback.append({'command': command, 'feedback': lines})
            (args.output / 'fixture-commands.json').write_text(json.dumps(feedback, indent=2) + '\n')
            if any(any(error in line for error in ('Unknown or incomplete', 'Could not ', 'Expected ', 'Incorrect argument', 'not loaded', 'Invalid ')) for line in lines):
                raise RuntimeError(f'fixture command refused: {command}: {lines}')
        source.send('save-all flush'); source.expect('Saved the game')
        source.stop()
        if source.process.returncode != 0:
            raise RuntimeError('source Java failed to stop cleanly')
        original = world_records(source_home / 'world')
        receipt['checks']['fixture'] = validate_fixture(original)
        native = args.output / 'native'
        native_port = runtime.reserve_port()
        config = args.output / 'server.toml'
        config.write_text(f'[server]\nview_distance=2\nsimulation_distance=2\npause_when_empty_seconds=1\n[network]\nbind_address="127.0.0.1"\nport={native_port}\n[auth]\nonline_mode=false\n[data]\nworld_dir="{native}"\nseed=42\nvanilla_data_dir="{args.cache}"\n')

        def convert(label, operation, src, dst):
            cmd = [str(args.binary), '--config', str(config), 'world', operation, '--source', str(src), '--destination', str(dst), '--receipt', str(args.output / (label + '.json'))]
            receipt['commands'].append(cmd)
            result = subprocess.run(cmd, cwd=args.output, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=600)
            (args.output / (label + '.log')).write_text(result.stdout)
            receipt['checks'][label] = {'exit': result.returncode}
            if result.returncode:
                raise RuntimeError(f'{label}: {result.stdout[-3000:]}')

        convert('import', 'import-anvil', source_home / 'world', native)
        unchanged = args.output / 'unchanged-export'
        convert('unchanged-export', 'export-anvil', native, unchanged)
        exported = world_records(unchanged)
        added = []
        diff = []
        omitted_protochunks = []
        for key, value in original.items():
            if key not in exported and isinstance(value, dict) and value.get('Status') not in (None, 'full', 'minecraft:full'):
                omitted_protochunks.append(key)
                continue
            diff.extend(differences(value, exported.get(key), key, added))
        import_receipt = json.loads((args.output / 'import.json').read_text())
        expected_omissions = import_receipt['counts']['non_full_empty_chunks']
        if len(omitted_protochunks) != expected_omissions:
            diff.append({'path': 'protochunk-omission-count', 'before': expected_omissions,
                         'after': len(omitted_protochunks)})
        comparison = {'differences': diff, 'added_fields': added, 'added_records': sorted(exported.keys() - original.keys()), 'omitted_protochunks': omitted_protochunks, 'source_records': len(original), 'exported_records': len(exported)}
        (args.output / 'normalized-diff.json').write_text(json.dumps(comparison, indent=2) + '\n')
        receipt['checks']['source-field-roundtrip'] = {'passed': not diff, 'difference_count': len(diff), 'omitted_protochunks': len(omitted_protochunks)}
        # Native gameplay is a separate leg, never folded into unchanged-world diff.
        solaris = launch(args.output / 'native-server', java=False, config=config)
        solaris.send('setblock 14 101 14 minecraft:emerald_block')
        solaris.send('save-all')
        solaris.expect('Saved the game')
        solaris.stop()
        if solaris.process.returncode != 0:
            raise RuntimeError('native server failed to stop cleanly')
        home = args.output / 'java-reopen'
        home.mkdir()
        convert('native-export', 'export-anvil', native, home / 'world')
        reopened = launch(home)
        lines = reopened.run('execute if block 14 101 14 minecraft:emerald_block run say ANVIL_NATIVE_BLOCK_OK')
        if not any('ANVIL_NATIVE_BLOCK_OK' in line for line in lines):
            raise RuntimeError(f'vanilla did not decode native block edit: {lines}')
        reopened.send('save-all flush'); reopened.expect('Saved the game')
        reopened.stop()
        if reopened.process.returncode != 0:
            raise RuntimeError('reopened Java failed to stop cleanly')
        errors = [line for line in reopened.log_path.read_text().splitlines() if '/ERROR]' in line]
        receipt['checks']['vanilla-reopen-save'] = {'passed': not errors, 'errors': errors}
        convert('reimport', 'import-anvil', home / 'world', args.output / 'reimported')
        receipt['passed'] = not diff and not errors
    except Exception as error:
        receipt['error'] = str(error)
    finally:
        for process in processes:
            runtime.stop_process(process)
        (args.output / 'result.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps({'passed': receipt['passed'], 'error': receipt.get('error'), 'report': str(args.output / 'result.json')}))
    return 0 if receipt['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
