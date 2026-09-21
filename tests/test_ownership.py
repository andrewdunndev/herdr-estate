import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from herdr_estate import ownership


class OwnershipRecords(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self._patch = mock.patch.dict(
            "os.environ", {"HERDR_PLUGIN_STATE_DIR": self._tmp.name})
        self._patch.start()
        self.addCleanup(self._patch.stop)

    def test_unrecorded_workspace_is_not_ours(self):
        self.assertFalse(ownership.is_ours("w1"))

    def test_record_then_is_ours(self):
        ownership.record("home/garden", "w1")
        self.assertTrue(ownership.is_ours("w1"))

    def test_record_persists_to_disk(self):
        ownership.record("home/garden", "w1")
        on_disk = json.loads((Path(self._tmp.name) / "owned.json").read_text())
        self.assertEqual(on_disk, {"w1": "home/garden"})

    def test_forget_drops_the_record(self):
        ownership.record("home/garden", "w1")
        ownership.forget("w1")
        self.assertFalse(ownership.is_ours("w1"))

    def test_forget_unrecorded_id_is_a_no_op(self):
        ownership.forget("never-recorded")  # must not raise

    def test_prune_drops_ids_not_in_the_live_set(self):
        ownership.record("home/garden", "w1")
        ownership.record("home/taxes", "w2")
        ownership.prune({"w2"})
        self.assertFalse(ownership.is_ours("w1"))
        self.assertTrue(ownership.is_ours("w2"))

    def test_prune_keeps_everything_when_all_still_exist(self):
        ownership.record("home/garden", "w1")
        ownership.prune({"w1"})
        self.assertTrue(ownership.is_ours("w1"))

    def test_corrupt_file_reads_as_empty(self):
        state = Path(self._tmp.name)
        state.mkdir(parents=True, exist_ok=True)
        (state / "owned.json").write_text("not json")
        self.assertFalse(ownership.is_ours("w1"))

    def test_non_object_json_reads_as_empty(self):
        state = Path(self._tmp.name)
        state.mkdir(parents=True, exist_ok=True)
        (state / "owned.json").write_text("[1, 2, 3]")
        self.assertFalse(ownership.is_ours("w1"))

    def test_write_leaves_no_temp_file_behind(self):
        ownership.record("home/garden", "w1")
        ownership.forget("w1")
        leftovers = list(Path(self._tmp.name).glob(".owned-*.tmp"))
        self.assertEqual(leftovers, [])


if __name__ == "__main__":
    unittest.main()
