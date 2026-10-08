"""Retirement selects one owned bundle and keeps unrelated Dock entries."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "retirement", Path(__file__).parents[1] / "retire-dodex-app.py")
retirement = importlib.util.module_from_spec(spec)
spec.loader.exec_module(retirement)


class RetirementSafety(unittest.TestCase):
    def test_dock_filter_removes_only_the_exact_managed_app(self):
        root = Path("/Users/example")
        app = root / "Applications/Dodex.app"
        tile = lambda url: {"tile-data": {"file-data": {"_CFURLString": url}}}
        preferences = {"persistent-apps": [
            tile(app.as_uri()), tile("file:///Applications/Codex.app/"),
            tile((root / "Applications/Unrelated Dodex.app").as_uri()),
        ], "autohide": True}
        updated = retirement.filtered_dock(preferences, app)
        self.assertEqual(updated["persistent-apps"], preferences["persistent-apps"][1:])
        self.assertTrue(updated["autohide"])
        self.assertEqual(len(preferences["persistent-apps"]), 3)

    def test_unmigrated_profile_cannot_authorize_desktop_retirement(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory).resolve()
            with self.assertRaises(FileNotFoundError):
                retirement.validate(home)
            self.assertEqual(list(home.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
