import unittest

from prune_exports import prune, uint


class ExportPruningTests(unittest.TestCase):
    def test_preserves_other_sections_and_required_exports(self):
        header = b"\0asm\x01\0\0\0"
        metadata = b"\x00\x04\x01xtt"
        enable = b"\x06enable\x00\x00"
        unused = b"\x04main\x00\x01"
        payload = b"\x02" + enable + unused
        source = header + metadata + b"\x07" + uint(len(payload)) + payload
        expected = header + metadata + b"\x07" + uint(len(enable) + 1) + b"\x01" + enable
        self.assertEqual(prune(source, {"enable"}), expected)

    def test_missing_required_callback_fails(self):
        with self.assertRaisesRegex(ValueError, "missing required Java exports"):
            prune(b"\0asm\x01\0\0\0", {"enable"})

    def test_truncated_section_fails(self):
        with self.assertRaisesRegex(ValueError, "truncated Wasm section"):
            prune(b"\0asm\x01\0\0\0\x07\x05\x00", set())

    def test_component_is_not_a_core_module(self):
        with self.assertRaisesRegex(ValueError, "expected a core Wasm module"):
            prune(b"\0asm\x0d\0\x01\0", set())


if __name__ == "__main__":
    unittest.main()
