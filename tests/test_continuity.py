import re
import tempfile
import unittest
from pathlib import Path

from herdr_estate.continuity import append, latest_conversation, read

# A line in the SAME FORMAT an existing estate continuity log already
# writes — timestamp, hostname, conversation id, space-separated — copied
# for its shape only, not its content, so a pre-existing sessions.log
# keeps resuming its latest conversation unmodified.
FIXTURE_LINE = ("2026-08-11T14:02:07.335219+00:00 example-host "
                 "3f9c2b7e-11aa-42bb-9cde-0123456789ab\n")


class EmptyLog(unittest.TestCase):
    def test_no_file_reads_empty(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(read(Path(d)), [])
            self.assertIsNone(latest_conversation(Path(d)))


class AppendReadRoundTrip(unittest.TestCase):
    def test_append_then_read(self):
        with tempfile.TemporaryDirectory() as d:
            cdir = Path(d)
            append(cdir, "sid-one")
            append(cdir, "sid-two")
            lines = read(cdir)
            self.assertEqual(len(lines), 2)
            self.assertEqual(latest_conversation(cdir), "sid-two")

    def test_line_shape_three_fields(self):
        with tempfile.TemporaryDirectory() as d:
            cdir = Path(d)
            append(cdir, "sid-one")
            line = read(cdir)[0]
            self.assertRegex(line, r"^\S+ \S+ \S+$")


class FixtureCompatibility(unittest.TestCase):
    def test_reads_a_preexisting_log_line(self):
        with tempfile.TemporaryDirectory() as d:
            cdir = Path(d)
            (cdir / "sessions.log").write_text(FIXTURE_LINE)
            self.assertEqual(latest_conversation(cdir),
                              "3f9c2b7e-11aa-42bb-9cde-0123456789ab")

    def test_appended_line_matches_fixture_shape(self):
        with tempfile.TemporaryDirectory() as d:
            cdir = Path(d)
            append(cdir, "new-sid")
            written = read(cdir)[0]
            fixture_fields = FIXTURE_LINE.strip().split()
            written_fields = written.split()
            self.assertEqual(len(written_fields), len(fixture_fields))
            # field 0: ISO-8601 timestamp
            self.assertRegex(written_fields[0],
                              r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}")
            # field 2: the conversation id, verbatim
            self.assertEqual(written_fields[2], "new-sid")


if __name__ == "__main__":
    unittest.main()
