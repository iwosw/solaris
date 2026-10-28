import unittest

from tools.worldgen_diff.profile import samples


class ProfileReceiptTests(unittest.TestCase):
    def test_reads_exact_three_seed_stage_rows(self):
        text = 'unrelated test output\n' + '\n'.join(
            f'STAGES seed={seed} run=0 chunks=36 ms_per_chunk=12.5 density_noise=8.0 '
            'surface_fill=1.0 carvers=0.0 features=3.0 structures=0.5 other=0.0 lighting=2.0'
            for seed in (0, 81, 497625))
        rows = samples(text)
        self.assertEqual([row['seed'] for row in rows], [0, 81, 497625])
        self.assertEqual(rows[0]['chunks'], 36)
        self.assertEqual(rows[1]['ms_per_chunk'], 12.5)

    def test_missing_samples_cannot_be_reported_as_a_speedup(self):
        with self.assertRaisesRegex(ValueError, 'missing or unexpected'):
            samples('test result: ok\n')


if __name__ == '__main__':
    unittest.main()
