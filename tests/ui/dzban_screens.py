# /// script
# requires-python = ">=3.9"
# dependencies = ["dogtail==2.1.0"]
# ///

import logging
import os
import tempfile
import time
import unittest
import warnings
from pathlib import Path

from dogtail.hermetic.session import HermeticSession, dump_tree, missing_binaries
from dogtail.logging import logging_class


APP_NAME = "Dzban"
MISSING_BINARIES = missing_binaries()
logging_class.logger.setLevel(logging.WARNING)
warnings.filterwarnings(
    "ignore",
    message="Atspi.Action.get_action_name is deprecated",
    category=DeprecationWarning,
)


def wait_for_node(root, name, role_name=None, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        nodes = root.find_children(
            lambda node: node.name == name
            and (role_name is None or node.role_name == role_name)
        )
        if nodes:
            return nodes[0]
        time.sleep(0.1)
    raise AssertionError(
        f"Did not find {name!r}"
        + (f" with role {role_name!r}" if role_name else "")
        + f":\n{dump_tree(root)}"
    )


def wait_for_node_containing(root, text, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        nodes = root.find_children(lambda node: text in (node.name or ""))
        if nodes:
            return nodes[0]
        time.sleep(0.1)
    raise AssertionError(f"Did not find text containing {text!r}:\n{dump_tree(root)}")


@unittest.skipIf(MISSING_BINARIES, f"missing: {', '.join(MISSING_BINARIES)}")
class DzbanScreens(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.binary = Path(os.environ["DZBAN_BINARY"]).resolve()
        cls.original_directory = Path.cwd()
        cls.session = HermeticSession(virtual_monitor="1440x900")
        cls.session.boot()

    @classmethod
    def tearDownClass(cls):
        os.chdir(cls.original_directory)
        cls.session.teardown()

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="dzban-ui-")
        os.chdir(self.directory.name)

    def tearDown(self):
        os.chdir(self.original_directory)
        self.directory.cleanup()

    def launch(self, config=None):
        if config is not None:
            Path("config.toml").write_text(config, encoding="utf-8")
        process = self.session.launch(
            [str(self.binary)],
            extra_environment={"RUST_LOG": "warn"},
        )
        self.addCleanup(process.wait, 5)
        self.addCleanup(process.terminate)
        return self.session.wait_for_app(APP_NAME)

    def test_empty_connections_and_shortcuts_screen(self):
        application = self.launch()

        wait_for_node(application, "No Connections Configured")
        wait_for_node(application, "Main Menu", "button").do_action_named("click")
        shortcuts = wait_for_node(application, "Keyboard shortcuts", "button")
        shortcuts.do_action_named("click")

        wait_for_node(application, "Keyboard Shortcuts")
        wait_for_node(application, "Estimate Query")
        wait_for_node(application, "Show Query Plan")

    def test_query_results_history_and_saved_plan(self):
        application = self.launch(
            """
[[databases]]
name = "Integration PostgreSQL"
host = "127.0.0.1"
port = 5415
user = "testuser"
database = "testdb"
password = "testpass"
""".strip()
        )

        wait_for_node(application, "Database Connections")
        wait_for_node(application, "Integration PostgreSQL")
        wait_for_node(application, "PostgreSQL database backend")
        wait_for_node(application, "Open", "button").do_action_named("click")
        wait_for_node(application, "Query History")

        action_field = wait_for_node(
            application, "Database search and query action", "text"
        )
        initial_editor = wait_for_node(application, "SQL query editor")
        self.assertEqual(initial_editor.text, "")
        for action in (
            "Focus Database Tabs",
            "Focus Query Tabs",
            "Focus Query Editor",
            "Focus Query Results",
            "Focus Query History",
        ):
            action_field.text = action
            wait_for_node(
                application, f"Run action: {action}", "button"
            ).do_action_named("click")
        action_field.text = "rename query"
        wait_for_node(application, "Rename Query Tab")
        action_field.text = "test all"
        wait_for_node(application, "Test All Connections")
        wait_for_node(application, "No Results")
        wait_for_node_containing(application, "PostgreSQL", timeout=15)
        wait_for_node(application, "Query execution time")
        wait_for_node(application, "Returned rows")
        wait_for_node(application, "Database ping latency")
        wait_for_node(application, "PostgreSQL database server")
        wait_for_node(application, "New query tab", "button").do_action_named("click")
        action_field.text = "query tab"
        wait_for_node(
            application, "Run action: Query tab: Query 2", "button"
        ).do_action_named("click")
        action_field.text = "rename query tab"
        wait_for_node(
            application, "Run action: Rename Query Tab", "button"
        ).do_action_named("click")
        rename_field = wait_for_node(application, "Query tab name", "text")
        rename_field.text = "Analysis"
        wait_for_node(application, "Rename", "button").do_action_named("click")
        wait_for_node(application, "Analysis")

        editor = wait_for_node(application, "SQL query editor")
        slow_query = (
            "SELECT value FROM generate_series(1, 3) value, "
            "LATERAL (SELECT pg_sleep(3)) delay"
        )
        editor.text = slow_query
        run_button = wait_for_node(application, "Run query", "button")
        run_button.do_action_named("click")
        wait_for_node(application, "Running Query", timeout=15)
        self.assertFalse(run_button.sensitive)
        run_button.do_action_named("click")
        wait_for_node_containing(application, "3 rows", timeout=15)
        wait_for_node(application, slow_query, timeout=15)
        self.assertTrue(run_button.sensitive)

        editor.text = "SELECT 1 AS answer"
        wait_for_node(application, "Run query", "button").do_action_named("click")

        wait_for_node_containing(application, "1 rows", timeout=15)
        wait_for_node(application, "SELECT 1 AS answer", timeout=15)

        long_value = "long-value-" * 40
        editor.text = (
            "SELECT 42::oid AS oid_value, '123'::xid AS xid_value, "
            "repeat('long-value-', 40) AS long_value"
        )
        wait_for_node(application, "Run query", "button").do_action_named("click")
        wait_for_node(application, "42", timeout=15)
        wait_for_node(application, "123", timeout=15)
        wait_for_node(application, long_value, timeout=15)

        plan_button = wait_for_node(application, "Show saved query plan", "button", timeout=15)
        plan_button.do_action_named("click")

        wait_for_node(application, "Query Plan")
        wait_for_node(application, "Database query plan")
        wait_for_node(application, "Unpin query plan", "button")
        wait_for_node(application, "Close query plan", "button").do_action_named("click")

        editor.text = "SELECT * FROM dzban_missing_table"
        wait_for_node(application, "Run query", "button").do_action_named("click")
        wait_for_node(application, "Query Failed", timeout=15)
        wait_for_node(application, "SELECT * FROM dzban_missing_table", timeout=15)

        for _ in range(2):
            action_field.text = "close query tab"
            wait_for_node(
                application, "Run action: Close Query Tab", "button"
            ).do_action_named("click")
        wait_for_node(application, "Database Connections")
        wait_for_node(application, "Open", "button").do_action_named("click")
        reopened_editor = wait_for_node(application, "SQL query editor")
        self.assertEqual(reopened_editor.text, "")

    def test_clickhouse_results_errors_history_and_plan(self):
        application = self.launch(
            """
[[databases]]
name = "Integration ClickHouse"
backend = "clickhouse"
url = "http://127.0.0.1:58123"
user = "dzban"
database = "dzban_test"
password = "dzban-test"
""".strip()
        )

        wait_for_node(application, "Integration ClickHouse")
        wait_for_node(application, "ClickHouse database backend")
        wait_for_node(application, "Open", "button").do_action_named("click")
        wait_for_node_containing(application, "25.", timeout=15)
        wait_for_node(application, "Query execution time")
        wait_for_node(application, "Returned rows")
        wait_for_node(application, "Database ping latency")
        wait_for_node(application, "ClickHouse database server")
        editor = wait_for_node(application, "SQL query editor")
        editor.text = "SELECT id, name, occurred_at FROM events ORDER BY id"
        wait_for_node(application, "Run query", "button").do_action_named("click")

        wait_for_node_containing(application, "3 rows", timeout=15)
        wait_for_node(application, "connected", timeout=15)
        wait_for_node(application, "2026-09-24 10:00:00.123", timeout=15)
        wait_for_node(
            application, "SELECT id, name, occurred_at FROM events ORDER BY id", timeout=15
        )
        wait_for_node(application, "Show saved query plan", "button").do_action_named(
            "click"
        )
        wait_for_node(application, "Query Plan")
        wait_for_node(application, "Database query plan")
        wait_for_node(application, "Unpin query plan", "button")
        wait_for_node(application, "Close query plan", "button").do_action_named("click")

        editor.text = "SELECT * FROM missing_clickhouse_table"
        wait_for_node(application, "Run query", "button").do_action_named("click")
        wait_for_node(application, "Query Failed", timeout=15)
        wait_for_node_containing(application, "missing_clickhouse_table", timeout=15)


if __name__ == "__main__":
    scenarios = {
        "empty": "test_empty_connections_and_shortcuts_screen",
        "database": "test_query_results_history_and_saved_plan",
        "clickhouse": "test_clickhouse_results_errors_history_and_plan",
    }
    scenario = os.environ.get("DZBAN_UI_SCENARIO")
    if scenario not in scenarios:
        raise SystemExit(f"DZBAN_UI_SCENARIO must be one of: {', '.join(scenarios)}")
    unittest.main(defaultTest=f"DzbanScreens.{scenarios[scenario]}", verbosity=2)
