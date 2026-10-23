import unittest

from tools.harness.anvil_interchange import differences, normalize, palette_cells


class AnvilDiffTests(unittest.TestCase):
    def test_palette_order_is_not_a_terrain_difference(self):
        a = {'Name': 'minecraft:oak_slab', 'Properties': {'type': 'top', 'waterlogged': 'true'}}
        b = {'Name': 'minecraft:stone'}
        one = {'palette': [a, b], 'data': [0x1010101010101010] * 256}
        two = {'palette': [b, a], 'data': [0x0101010101010101] * 256}
        self.assertEqual(normalize({'block_states': one}), normalize({'block_states': two}))
        changed = {'palette': [dict(a, Properties={'type': 'top', 'waterlogged': 'false'}), b], 'data': one['data']}
        self.assertNotEqual(normalize({'block_states': one}), normalize({'block_states': changed}))

    def test_missing_metadata_fails_while_added_defaults_are_enumerated(self):
        before = {'CustomName': {'text': 'Chest'}, 'Items': []}
        additions = []
        self.assertEqual(differences(before, dict(before, TransferCooldown=0), additions=additions), [])
        self.assertEqual(additions, ['/TransferCooldown'])
        self.assertEqual(differences(before, {'Items': []})[0]['path'], '/CustomName')

    def test_three_dimensional_biome_indices_preserve_vertical_variation(self):
        values = palette_cells({'palette': ['plains', 'lush_caves'], 'data': [0xffff0000ffff0000]}, 64, 1)
        self.assertEqual(values[:16], ['plains'] * 16)
        self.assertEqual(values[16:32], ['lush_caves'] * 16)
        self.assertEqual(values[32:48], ['plains'] * 16)

    def test_sign_message_order_is_not_normalized_away(self):
        a = {'front_text': {'messages': ['first', 'second']}, 'back_text': {'messages': ['back']}}
        b = {'front_text': {'messages': ['second', 'first']}, 'back_text': {'messages': ['back']}}
        self.assertTrue(differences(normalize(a), normalize(b)))


if __name__ == '__main__':
    unittest.main()
