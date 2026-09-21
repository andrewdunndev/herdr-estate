import unittest

from herdr_estate.frontmatter import parse_frontmatter


class ScalarsAndLists(unittest.TestCase):
    def test_no_fence_is_empty(self):
        self.assertEqual(parse_frontmatter("# garden\n\nbody text\n"), {})

    def test_scalar(self):
        text = "---\nagent: claude\n---\n"
        self.assertEqual(parse_frontmatter(text), {"agent": "claude"})

    def test_flow_list(self):
        text = "---\npaths: [~/code/a, ~/code/b]\n---\n"
        self.assertEqual(parse_frontmatter(text)["paths"], ["~/code/a", "~/code/b"])

    def test_empty_flow_list(self):
        text = "---\npaths: []\n---\n"
        self.assertEqual(parse_frontmatter(text)["paths"], [])

    def test_block_list(self):
        text = "---\npaths:\n  - ~/code/a\n  - ~/code/b\n---\n"
        self.assertEqual(parse_frontmatter(text)["paths"], ["~/code/a", "~/code/b"])

    def test_inline_comment_stripped(self):
        text = "---\nagent: claude  # default kind\n---\n"
        self.assertEqual(parse_frontmatter(text)["agent"], "claude")

    def test_quotes_stripped(self):
        text = '---\nagent: "claude"\n---\n'
        self.assertEqual(parse_frontmatter(text)["agent"], "claude")


class NestedTables(unittest.TestCase):
    def test_kind_table(self):
        text = ("---\nclaude:\n  remote-control: true\n"
                "  mcp-config: .mcp.json\n---\n")
        data = parse_frontmatter(text)
        self.assertEqual(data["claude"], {"remote-control": True,
                                           "mcp-config": ".mcp.json"})

    def test_worktree_table(self):
        text = ("---\nworktree:\n  repo: ~/code/site\n"
                "  branch: wt/site-work\n---\n")
        data = parse_frontmatter(text)
        self.assertEqual(data["worktree"], {"repo": "~/code/site",
                                             "branch": "wt/site-work"})

    def test_boolean_false(self):
        text = "---\nclaude:\n  remote-control: false\n---\n"
        self.assertIs(parse_frontmatter(text)["claude"]["remote-control"], False)


class UnknownKeysTolerated(unittest.TestCase):
    def test_unknown_keys_kept_not_rejected(self):
        text = ("---\nagent: claude\nnotes: something a future tool reads\n"
                "paths: []\n---\n")
        data = parse_frontmatter(text)
        self.assertEqual(data["agent"], "claude")
        self.assertIn("notes", data)  # kept, not filtered — caller ignores it

    def test_no_crash_on_malformed_line(self):
        text = "---\n: no key here\nagent: claude\n---\n"
        data = parse_frontmatter(text)
        self.assertEqual(data.get("agent"), "claude")


if __name__ == "__main__":
    unittest.main()
