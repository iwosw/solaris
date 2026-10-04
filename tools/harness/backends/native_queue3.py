"""Windowed native Loader UI and milk/food probes; lifecycle remains harness-owned."""
import json

from PIL import Image, ImageChops
from . import native_client
from .probe_steps import ProbeSteps, item_count


def changed_region(before, after, region, after_region=None):
    a = Image.open(before['retained_path']).convert('RGB')
    b = Image.open(after['retained_path']).convert('RGB')
    if a.size != b.size or not before['gpu_completion'] or not after['gpu_completion']:
        raise RuntimeError('matching completed graphical frames required')
    after_region = region if after_region is None else after_region
    diff = ImageChops.difference(a.crop(region), b.crop(after_region))
    changed = sum(max(pixel) >= 16 for pixel in diff.getdata())
    return {'region': region, 'after_region': after_region, 'changed_pixels': changed,
            'total_pixels': diff.width * diff.height,
            'before_frame': before['frame_id'], 'after_frame': after['frame_id']}


def run_loader_ui(client, artifact, timeout):
    with ProbeSteps(client, artifact / 'loader-ui-proof', timeout, 'native10-generic-ui-loader') as steps:
        admitted = steps.wait(lambda s: s.get('loader', {}).get('acknowledged') is True, 'verified_bundle_admitted')
        steps.check('native_platform', admitted['loader']['platform'] == 'native')
        steps.check('declared_view', 'ruby-live:showcase' in admitted['loader']['declared_views'])
        steps.check('verified_asset', 'ruby-live:fixture_icon' in admitted['loader']['activated_assets'])
        steps.call('minecraft_send_chat', {'message': 'loader_ruby', 'command': True})
        opened = steps.wait(lambda s: s['screen']['class'] == 'dev.solaris.loader.minecraft.LoaderViewScreen', 'received_loader_view')
        opened_frame = native_client.capture(client, artifact, 'loader-open')
        steps.call('minecraft_set_screen_input', {'expected_screen_class': opened['screen']['class'],
            'expected_title': opened['screen']['title'], 'input_label': 'Name', 'value': ''})
        before = native_client.capture(client, artifact, 'loader-focused-empty')
        steps.call('minecraft_set_screen_input', {'expected_screen_class': opened['screen']['class'],
            'expected_title': opened['screen']['title'], 'input_label': 'Name', 'value': 'Native UI proof'})
        after = native_client.capture(client, artifact, 'loader-input')
        width, height = Image.open(after['retained_path']).size
        # Exact schema fixture: five 12px lines with 4px gaps, then 160x20 Name field at y88.
        # Both captures focus the same field; exclude its border/caret from the glyph predicate.
        region = (width//2 - 74, 92, width//2 + 74, 104)
        predicate = changed_region(before, after, region)
        steps.check('input_visible_changed_pixels', predicate['changed_pixels'] >= 20)
        steps.call('minecraft_click_screen_button', {'expected_screen_class': opened['screen']['class'],
            'expected_title': opened['screen']['title'], 'button_label': 'Confirm native input'})
        confirmed = steps.wait(lambda s: any('Native UI proof' in line for line in s['recent_chat']), 'server_observed_name')
        steps.report.update(scope='generic ruby-live UI-only fixture, not full Settlements', admitted=admitted['loader'],
            screen=opened['screen'], submitted_state=confirmed, captures={'opened': opened_frame, 'before': before, 'after': after}, pixel_predicates=predicate)
    return steps.report


def run_milk_food(client, artifact, timeout):
    with ProbeSteps(client, artifact / 'milk-food-proof', timeout, 'native10-windowed-milk-food') as steps:
        steps.report.update(scope='ordinary milk/food use; fixture items and hunger are privileged, not earned production',
            setup='explicit debug give/exhaust; no hidden minting or plugin quality claims')
        initial = steps.wait(lambda s: s['player']['on_ground'], 'grounded_player')
        steps.fixture('debug give minecraft:milk_bucket 1 0')
        steps.call('minecraft_wait_for_inventory', {'item_id': 'minecraft:milk_bucket', 'count': 1, 'timeout_seconds': steps.remaining()})
        steps.call('minecraft_set_hotbar_slot', {'slot': 0})
        steps.call('minecraft_look', {'yaw_deg': 0, 'pitch_deg': -90})
        milk_before = native_client.capture(client, artifact, 'milk-before')
        steps.call('minecraft_press_inputs', {'keys': ['use'], 'ticks': 40})
        milk = steps.wait(lambda s: s['selected_item']['item_id'] == 'minecraft:bucket'
            and item_count(s, 'minecraft:milk_bucket') == 0, 'received_milk_remainder')
        steps.check('exact_empty_bucket', item_count(milk, 'minecraft:bucket') == 1)
        milk_after = native_client.capture(client, artifact, 'milk-after')
        steps.fixture('debug survival exhaust 80')
        hunger = steps.wait(lambda s: s['player']['food'] <= 14, 'received_fixture_hunger')
        steps.fixture('debug give minecraft:apple 1 0')
        steps.call('minecraft_wait_for_inventory', {'item_id': 'minecraft:apple', 'count': 1, 'timeout_seconds': steps.remaining()})
        food_before_state = steps.observe()
        food_before = native_client.capture(client, artifact, 'food-before')
        steps.call('minecraft_press_inputs', {'keys': ['use'], 'ticks': 40})
        food = steps.wait(lambda s: item_count(s, 'minecraft:apple') == 0
            and s['player']['food'] > food_before_state['player']['food'], 'received_food_consumption')
        steps.check('apple_four_food_points', food['player']['food'] == food_before_state['player']['food'] + 4)
        food_after = native_client.capture(client, artifact, 'food-after')
        width, height = Image.open(milk_before['retained_path']).size
        # Source hud_tree slot0's 16x16 icon, not the moving terrain or tooltip.
        hotbar = (width//2 - 88, height - 19, width//2 - 72, height - 3)
        predicates = {'milk_selected_icon': changed_region(milk_before, milk_after, hotbar),
                      'food_selected_icon': changed_region(food_before, food_after, hotbar)}
        for name, predicate in predicates.items():
            steps.check(name + '_pixels_changed', predicate['changed_pixels'] >= 20)
        steps.check('same_player_dimension', initial['player']['entity_uuid'] == food['player']['entity_uuid']
            and initial['dimension'] == food['dimension'])
        steps.report.update(milk_state=milk, food_before=food_before_state, food_state=food, pixel_predicates=predicates,
            captures={'milk_before': milk_before, 'milk_after': milk_after, 'food_before': food_before, 'food_after': food_after})
    return steps.report


def run_full_bundle(client, artifact, timeout, index, setup_commands, view_key=None, scope='sealed full bundle admission, UI/model rendering only; not village or quality acceptance'):
    """Admit/capture the sealed candidate; isolated setup is not gameplay acceptance."""
    with ProbeSteps(client, artifact / 'full-bundle-proof', timeout, 'native10-full-bundle-capture') as steps:
        steps.report.update(scope=scope,
            setup_commands=setup_commands, item_fixture='privileged give, not manufactured or earned production')
        admitted = steps.wait(lambda s: s.get('loader', {}).get('acknowledged') is True, 'verified_full_bundle_admitted')
        for section, observation in (
                ('screens', 'declared_views'), ('items', 'registered_items'),
                ('assets', 'activated_assets'), ('world_previews', 'world_previews'),
                ('sounds', 'registered_sounds')):
            declared = {entry['id'] for entry in index.get(section, [])}
            received = set(admitted['loader'][observation])
            steps.check('sealed_' + section + '_admitted', declared <= received)
        steps.check('native_platform', admitted['loader']['platform'] == 'native')
        for command in setup_commands:
            steps.call('minecraft_send_chat', {'message': command, 'command': True})
        overview = next(screen for screen in index['screens'] if screen['id'] == view_key) if view_key else next(screen for screen in index['screens'] if screen['id'].endswith(':overview'))
        steps.call('minecraft_request_loader_view', {'view_key': overview['id']})
        opened = steps.wait(lambda s: (s['loader'].get('view') or {}).get('view_id') == overview['id']
            and s['screen']['class'] == 'dev.solaris.loader.minecraft.LoaderViewScreen', 'received_full_overview')
        overview_frame = native_client.capture(client, artifact, 'full-overview')
        steps.call('minecraft_close_screen')
        steps.wait(lambda s: s['screen']['class'] != 'dev.solaris.loader.minecraft.LoaderViewScreen', 'full_overview_closed')
        item = index['items'][0]
        steps.fixture('debug give ' + item['base_item'] + ' 1 0')
        steps.call('minecraft_wait_for_inventory', {'item_id': item['base_item'], 'count': 1, 'timeout_seconds': steps.remaining()})
        steps.call('minecraft_set_hotbar_slot', {'slot': 0})
        steps.call('minecraft_look', {'yaw_deg': 0, 'pitch_deg': -90})
        baseline = steps.observe()
        # Only hotbar occupancy determines the newly received model icon.
        # Native auxiliary cells 41/42 may truthfully remain unknown.
        occupied = {stack['slot'] for stack in baseline['inventory']
            if 0 <= stack['slot'] <= 8 and int(stack['count']) > 0}
        before = native_client.capture(client, artifact, 'full-vanilla-carrier')
        steps.fixture('give ' + item['base_item'] + '[minecraft:item_model=' + json.dumps(item['id']) + '] 1')
        received = steps.wait(lambda s: item_count(s, item['base_item']) == item_count(baseline, item['base_item']) + 1,
            'received_privileged_model_fixture')
        slots = [stack['slot'] for stack in received['inventory'] if stack['item_id'] == item['base_item']
            and int(stack['count']) == 1 and stack['slot'] not in occupied and 0 <= stack['slot'] <= 8]
        steps.check('unique_new_hotbar_model_stack', len(slots) == 1)
        slot = slots[0]
        steps.call('minecraft_set_hotbar_slot', {'slot': slot})
        components = steps.call('minecraft_read_item_components', {'source': 'selected'})
        steps.check('exact_received_item_model', components['components'].get('minecraft:item_model') == item['id'])
        after = native_client.capture(client, artifact, 'full-plugin-item-model')
        width, height = Image.open(before['retained_path']).size
        vanilla_icon = (width//2 - 88, height - 19, width//2 - 72, height - 3)
        model_icon = (vanilla_icon[0] + slot * 20, vanilla_icon[1], vanilla_icon[2] + slot * 20, vanilla_icon[3])
        predicate = changed_region(before, after, vanilla_icon, model_icon)
        steps.check('declared_model_differs_from_vanilla_carrier', predicate['changed_pixels'] >= 20)
        steps.report.update(admitted=admitted['loader'], screen=opened['screen'], received_item_components=components,
            pixel_predicates=predicate, captures={'overview': overview_frame, 'vanilla_carrier': before, 'plugin_model': after})
    return steps.report
