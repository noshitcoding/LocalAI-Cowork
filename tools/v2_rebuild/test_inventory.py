import json
import tempfile
import unittest
from pathlib import Path

from inventory import build_inventory


class InventoryTests(unittest.TestCase):
    def test_every_requirement_is_accounted_for_without_asserting_qualification(self):
        root = Path(__file__).resolve().parents[2]
        inventory = build_inventory(root)
        self.assertEqual({row["id"] for row in inventory["requirements"]}, set(range(1, 88)))
        self.assertFalse(inventory["production_accepted"])
        for row in inventory["requirements"]:
            self.assertEqual(row["evidence"], [])
            self.assertIsNone(row["delivery_artifact"])
            if row["id"] != 78:
                self.assertEqual(row["status"], "requires_current_verification")

    def test_new_or_duplicate_requirement_cannot_be_silently_omitted(self):
        root = Path(__file__).resolve().parents[2]
        matrix = (root / "docs/FEATURE-MATRIX.md").read_text(encoding="utf-8-sig")
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory)
            (fixture / "docs").mkdir()
            for suffix in ["\n| 88 | New requirement | Needs a decision |\n", "\n| 1 | Duplicate | Must not hide an original |\n"]:
                (fixture / "docs/FEATURE-MATRIX.md").write_text(matrix + suffix, encoding="utf-8")
                with self.assertRaises(ValueError):
                    build_inventory(fixture)


if __name__ == "__main__":
    unittest.main()
