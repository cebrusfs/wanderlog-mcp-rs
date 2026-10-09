"""Regression tests for offline release metadata safeguards."""
from pathlib import Path
import runpy
import shutil
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
check = runpy.run_path(str(ROOT / 'scripts/release-check.py'))['check']


class ReleaseChecks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for path in ('Cargo.toml', 'LICENSE', 'NOTICE',
                     'crates/wanderlog-client/Cargo.toml',
                     'crates/wanderlog-client/LICENSE', 'crates/wanderlog-client/NOTICE'):
            dest = self.root / path
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / path, dest)

    def replace(self, path, old, new):
        file = self.root / path
        content = file.read_text()
        self.assertIn(old, content)
        file.write_text(content.replace(old, new, 1))

    def test_repository_metadata_is_consistent(self):
        version = check(self.root)
        self.assertEqual(check(self.root, 'v' + version), version)

    def test_wrong_tag_is_rejected(self):
        with self.assertRaises(ValueError):
            check(self.root, 'v999.999.999')

    def test_wrong_client_name_is_rejected(self):
        self.replace('crates/wanderlog-client/Cargo.toml',
                     'name = "wanderlog-client"', 'name = "unintended-name"')
        with self.assertRaises(ValueError):
            check(self.root)

    def test_client_version_drift_is_rejected(self):
        version = check(self.root)
        self.replace('crates/wanderlog-client/Cargo.toml',
                     'version = "' + version + '"', 'version = "999.999.999"')
        with self.assertRaises(ValueError):
            check(self.root)

    def test_application_dependency_leak_is_rejected(self):
        self.replace('crates/wanderlog-client/Cargo.toml',
                     '[dependencies]', '[dependencies]\nrmcp = "3"')
        with self.assertRaises(ValueError):
            check(self.root)

    def test_notice_mismatch_is_rejected(self):
        with (self.root / 'crates/wanderlog-client/NOTICE').open('a') as handle:
            handle.write('Unexpected modification\n')
        with self.assertRaises(ValueError):
            check(self.root)

    def test_license_drift_is_rejected(self):
        self.replace('Cargo.toml', 'license = "Apache-2.0"', 'license = "MIT"')
        with self.assertRaises(ValueError):
            check(self.root)


if __name__ == '__main__':
    unittest.main()
