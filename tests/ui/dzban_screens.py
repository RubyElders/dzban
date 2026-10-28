# /// script
# requires-python = ">=3.9"
# dependencies = ["dogtail==2.1.0"]
# ///

import json
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

POSTGRES = """
[[databases]]
name = "Integration PostgreSQL"
host = "127.0.0.1"
port = 5415
user = "testuser"
database = "testdb"
password = "testpass"
""".strip()

CLICKHOUSE = """
[[databases]]
name = "Integration ClickHouse"
backend = "clickhouse"
url = "http://127.0.0.1:58123"
user = "dzban"
database = "dzban_test"
password = "dzban-test"
""".strip()


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


def wait_for_any(root, names, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        nodes = root.find_children(lambda node: node.name in names)
        if nodes:
            return nodes[0]
        time.sleep(0.1)
    raise AssertionError(f"Did not find any of {names!r}:\n{dump_tree(root)}")


def wait_for_node_containing(root, text, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        nodes = root.find_children(lambda node: text in (node.name or ""))
        if nodes:
            return nodes[0]
        time.sleep(0.1)
    raise AssertionError(f"Did not find text containing {text!r}:\n{dump_tree(root)}")


def wait_for_node_to_disappear(root, name, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not root.find_children(lambda node: node.name == name):
            return
        time.sleep(0.1)
    raise AssertionError(f"{name!r} remained visible:\n{dump_tree(root)}")


def wait_until(predicate, message, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.1)
    raise AssertionError(message)


@unittest.skipIf(MISSING_BINARIES, f"missing: {', '.join(MISSING_BINARIES)}")
class DzbanScreens(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.binary = Path(os.environ["DZBAN_BINARY"]).resolve()
        cls.original_directory = Path.cwd()
        cls.session = HermeticSession(virtual_monitor="1440x900")
        cls.session.boot()
        input_backend = cls.session.install_input()
        input_backend.connectMonitor()
        time.sleep(0.3)

    @classmethod
    def tearDownClass(cls):
        os.chdir(cls.original_directory)
        cls.session.teardown()

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="dzban-ui-")
        os.chdir(self.directory.name)
        self.data = Path(self.directory.name) / "data"
        self.data.mkdir()

    def tearDown(self):
        os.chdir(self.original_directory)
        self.directory.cleanup()

    def launch(self, config=None):
        if config is not None:
            Path("config.toml").write_text(config, encoding="utf-8")
        process = self.session.launch(
            [str(self.binary)],
            extra_environment={"RUST_LOG": "warn", "XDG_DATA_HOME": str(self.data)},
        )
        self.process = process
        self.addCleanup(process.wait, 5)
        self.addCleanup(process.terminate)
        application = self.session.wait_for_app(APP_NAME)
        from gi.repository import Atspi

        application.set_cache_mask(Atspi.Cache.NONE)
        return application

    def action(self, application, query, title):
        field = wait_for_node(application, "Database search and query action", "text")
        field.text = query
        wait_for_node(application, f"Run action: {title}", "button").do_action_named("click")

    def run_sql(self, application, sql):
        editor = wait_for_node(application, "SQL query editor")
        editor.text = sql
        wait_for_node(application, "Run query", "button").do_action_named("click")
        return editor

    def test_busy_window_close_cancels_work_and_saves_tabs(self):
        from dogtail import rawinput

        application = self.launch(POSTGRES)
        wait_for_node(application, "Open", "button").do_action_named("click")
        self.run_sql(application, "SELECT pg_sleep(30)")
        wait_for_node(application, "Running query")
        wait_for_node(application, "Stop query", "button")

        rawinput.key_combo("<Alt>F4")
        self.assertEqual(self.process.wait(timeout=15), 0)
        saved = json.loads((self.data / "dzban" / "gtk-workspace.json").read_text())
        self.assertEqual(saved["tabs"][0]["profile"], "Integration PostgreSQL")
        self.assertEqual(saved["tabs"][0]["draft"], "SELECT pg_sleep(30)")

    def test_empty_connections_and_shortcuts_screen(self):
        application = self.launch()

        wait_for_node(application, "No connection profiles found")
        wait_for_node(application, "Reload Profiles", "button")
        wait_for_node(application, "New Query")
        wait_for_node(application, "Keyboard shortcuts", "button").do_action_named("click")
        wait_for_node(application, "Keyboard Shortcuts")
        wait_for_node(application, "Estimate Query")
        wait_for_node(application, "Preview Query Plan")
        wait_for_node(application, "Ctrl+Enter")

    def test_query_results_history_and_saved_plan(self):
        from dogtail import rawinput
        from dogtail.config import config as dogtail_config

        application = self.launch(POSTGRES)

        wait_for_node(application, "Database Connections")
        wait_for_node(application, "Integration PostgreSQL")
        wait_for_node(application, "PostgreSQL database backend")
        wait_for_node(application, "Open", "button").do_action_named("click")
        wait_for_node(application, "Ready to query")
        wait_for_node(application, "Integration PostgreSQL - Query 1")
        wait_for_node_containing(application, "PostgreSQL 15", timeout=15)
        wait_for_node(application, "Query execution time")
        wait_for_node(application, "Database ping latency")
        wait_for_node(application, "PostgreSQL database server")

        history_toggle = wait_for_node(application, "Toggle query history", "button")
        history_toggle.do_action_named("click")
        wait_for_node(application, "Query History")
        history_toggle.do_action_named("click")
        wait_for_node_to_disappear(application, "Query History")

        initial_editor = wait_for_node(application, "SQL query editor")
        self.assertEqual(initial_editor.text, "")
        for action in (
            "Focus Query Tabs",
            "Focus Query Editor",
            "Focus Results",
            "Focus Query History",
        ):
            self.action(application, action, action)
        self.action(application, "rename query", "Rename Query Tab")
        rename_field = wait_for_node(application, "Query tab name", "text")
        rename_field.text = "Analysis"
        wait_for_node(application, "Rename", "button").do_action_named("click")
        wait_for_node(application, "Integration PostgreSQL - Analysis")

        for sql, affected in [
            ("BEGIN", 0),
            ("CREATE TEMP TABLE dzb_gtk_ack (value integer)", 0),
            ("INSERT INTO dzb_gtk_ack VALUES (1), (2)", 2),
            ("ROLLBACK", 0),
        ]:
            self.run_sql(application, sql)
            wait_for_node(application, "Command complete", timeout=15)
            wait_for_node(application, f"{affected} rows affected", timeout=15)

        editor = wait_for_node(application, "SQL query editor")
        editor.text = "SELECT 1 AS preview_value"
        editor.grab_focus()
        for key in ("Control_L", "Control_R", "Shift_L", "space"):
            self.addCleanup(rawinput.release_key, key)
        dogtail_config.action_delay = 0.02
        dogtail_config.typing_delay = 0.02
        rawinput.hold_key("Control_L")
        rawinput.hold_key("Control_R")
        rawinput.release_key("Control_L")
        rawinput.hold_key("Shift_L")
        field = wait_for_node(application, "Database search and query action", "text")
        wait_until(
            lambda: "Space for plan" in (field.description or ""),
            f"The estimate hint did not appear: {field.description!r}",
            timeout=15,
        )
        rawinput.hold_key("space")
        wait_for_node(application, "Query Plan")
        time.sleep(0.6)
        rawinput.release_key("space")
        wait_for_node_to_disappear(application, "Query Plan")
        rawinput.release_key("Shift_L")
        rawinput.release_key("Control_R")

        rawinput.hold_key("Control_L")
        rawinput.hold_key("Shift_L")
        field = wait_for_node(application, "Database search and query action", "text")
        wait_until(
            lambda: "Space for plan" in (field.description or ""),
            f"The estimate hint did not appear: {field.description!r}",
            timeout=15,
        )
        rawinput.press_key("space")
        rawinput.release_key("Shift_L")
        rawinput.release_key("Control_L")
        wait_for_node(application, "Query Plan")
        wait_for_node(application, "Unpin query plan", "button")
        wait_for_node(application, "Database query plan")
        wait_for_node(application, "Close query plan", "button").do_action_named("click")
        wait_for_node_to_disappear(application, "Query Plan")

        slow_query = "SELECT pg_sleep(30), 1 AS value"
        self.run_sql(application, slow_query)
        wait_for_node(application, "Running query", timeout=15)
        stop_button = wait_for_node(application, "Stop query", "button")
        self.assertTrue(stop_button.sensitive)

        self.action(application, "show connections", "Show Connections")
        wait_for_node(application, "Database Connections")
        wait_for_node(application, "New Query")
        self.assertFalse(wait_for_node(application, "Run query", "button").sensitive)
        self.action(
            application,
            "Analysis",
            "Tab: Integration PostgreSQL - Analysis",
        )
        stop_button = wait_for_node(application, "Stop query", "button")
        self.assertTrue(stop_button.sensitive)
        stop_button.do_action_named("click")
        wait_for_any(application, ("Query stopped", "Query failed"), timeout=15)
        self.assertTrue(wait_for_node(application, "Run query", "button").sensitive)

        self.run_sql(application, slow_query)
        wait_for_node(application, "Running query", timeout=15)
        wait_for_node(application, "SQL query editor").grab_focus()
        rawinput.press_key("Escape")
        wait_for_any(application, ("Query stopped", "Query failed"), timeout=15)
        self.assertTrue(wait_for_node(application, "Run query", "button").sensitive)

        self.run_sql(application, "SELECT 1 AS answer")
        wait_for_node(application, "Complete - 1 rows", timeout=15)

        long_value = "long-value-" * 40
        self.run_sql(
            application,
            "SELECT 42::oid AS oid_value, '123'::xid AS xid_value, "
            "repeat('long-value-', 40) AS long_value",
        )
        wait_for_node(application, "42", timeout=15)
        wait_for_node(application, "123", timeout=15)
        wait_for_node(application, long_value, timeout=15)

        self.run_sql(application, "SELECT * FROM dzban_missing_table")
        wait_for_node(application, "Query failed", timeout=15)
        wait_for_node_containing(application, "dzban_missing_table", timeout=15)

        self.action(application, "focus query history", "Focus Query History")
        wait_for_node(application, "SELECT 1 AS answer", "list item", timeout=15)
        rawinput.press_key("Down")
        rawinput.press_key("Down")
        rawinput.press_key("Return")
        wait_for_node(application, "Saved - 1 of 1 rows", timeout=15)
        wait_for_node(application, "Text sample")
        wait_for_node(application, "Saved Plan", "button").do_action_named("click")
        wait_for_node(application, "Query Plan")
        wait_for_node(application, "Database query plan")
        wait_for_node(application, "Close query plan", "button").do_action_named("click")

        self.action(application, "close query tab", "Close Query Tab")
        wait_for_node_to_disappear(application, "Integration PostgreSQL - Analysis")
        wait_for_node(application, "Database Connections")
        wait_for_node(application, "Open", "button").do_action_named("click")
        reopened_editor = wait_for_node(application, "SQL query editor")
        self.assertEqual(reopened_editor.text, "")

    def test_clickhouse_results_errors_history_and_plan(self):
        from dogtail import rawinput

        application = self.launch(CLICKHOUSE)

        wait_for_node(application, "Integration ClickHouse")
        wait_for_node(application, "ClickHouse database backend")
        wait_for_node(application, "Open", "button").do_action_named("click")
        wait_for_node_containing(application, "25.", timeout=15)
        wait_for_node(application, "Query execution time")
        wait_for_node(application, "ClickHouse database server")
        self.run_sql(application, "SET max_threads = 2")
        wait_for_node(application, "Command complete", timeout=15)
        wait_for_node(application, "Command completed", timeout=15)

        query = "SELECT id, name, occurred_at FROM events ORDER BY id"
        self.run_sql(application, query)
        wait_for_node(application, "Complete - 3 rows", timeout=15)
        wait_for_node(application, "connected", timeout=15)
        wait_for_node(application, "2026-09-24 10:00:00.123", timeout=15)

        self.run_sql(application, "SELECT * FROM missing_clickhouse_table")
        wait_for_node(application, "Query failed", timeout=15)
        wait_for_node_containing(application, "missing_clickhouse_table", timeout=15)

        self.action(application, "focus query history", "Focus Query History")
        wait_for_node(application, query, "list item", timeout=15)
        rawinput.press_key("Down")
        rawinput.press_key("Return")
        wait_for_node(application, "Saved - 3 of 3 rows", timeout=15)
        wait_for_node(application, "Saved Plan", "button").do_action_named("click")
        wait_for_node(application, "Query Plan")
        wait_for_node(application, "Database query plan")
        wait_for_node(application, "Unpin query plan", "button")
        wait_for_node(application, "Close query plan", "button").do_action_named("click")

    def check_streamed_results(self, backend):
        from dogtail import rawinput
        from dogtail.config import config as dogtail_config

        dogtail_config.action_delay = 0.02
        dogtail_config.typing_delay = 0.02

        endpoint = (
            'host = "127.0.0.1"\nport = 5415'
            if backend == "postgresql"
            else 'url = "http://127.0.0.1:58123"'
        )
        credentials = (
            'user = "testuser"\ndatabase = "testdb"\npassword = "testpass"'
            if backend == "postgresql"
            else 'user = "dzban"\ndatabase = "dzban_test"\npassword = "dzban-test"'
        )
        application = self.launch(
            f'[[databases]]\nname = "Streaming fixture"\nbackend = "{backend}"\n'
            f"{endpoint}\n{credentials}"
        )
        rawinput.press_key("F11")
        frame = wait_for_node(application, "Dzban", "frame")
        wait_until(
            lambda: tuple(frame.extents[2:]) == (1440, 900),
            f"The window did not become fullscreen: {frame.extents}",
        )
        wait_for_node(application, "Open", "button").do_action_named("click")
        wait_for_node_containing(
            application, "15." if backend == "postgresql" else "25.", timeout=15
        )
        self.run_sql(
            application,
            "SELECT i AS stream_value, repeat('x', 9000) AS payload, pg_sleep(0.1) "
            "FROM generate_series(1, 100) AS i"
            if backend == "postgresql"
            else "SELECT number + 1 AS stream_value, repeat('x', 9000) AS payload, "
            "sleepEachRow(0.1) FROM numbers(100) SETTINGS max_block_size=1, max_threads=1",
        )
        wait_for_node_containing(application, "Receiving - ", timeout=15)
        wait_for_node(application, "1", timeout=15)
        self.assertTrue(wait_for_node(application, "Stop query", "button").sensitive)
        wait_for_node(application, "SQL query editor").grab_focus()
        rawinput.press_key("Escape")
        wait_for_node_containing(application, "Canceled - ", timeout=15)

        self.run_sql(
            application,
            "SELECT generate_series(1, 12000) AS capped_value"
            if backend == "postgresql"
            else "SELECT number AS capped_value FROM numbers(12000)",
        )
        wait_for_node(application, "Truncated - 10000 rows (row limit)", timeout=20)
        self.assertTrue(wait_for_node(application, "Run query", "button").sensitive)

        self.run_sql(
            application,
            "SELECT repeat('x', 1048576) AS byte_limited FROM generate_series(1, 40)"
            if backend == "postgresql"
            else "SELECT repeat('xx', 524288) AS byte_limited FROM numbers(40)",
        )
        wait_for_node(application, "Truncated - 31 rows (byte limit)", timeout=25)
        self.assertTrue(wait_for_node(application, "Run query", "button").sensitive)

        full_value = "full-value-" * 100
        self.run_sql(application, "SELECT repeat('full-value-', 100) AS complete_value")
        wait_for_node(application, "Complete - 1 rows", timeout=15)
        results = wait_for_node(application, "Query results")
        deadline = time.monotonic() + 15
        cells = []
        while not cells and time.monotonic() < deadline:
            cells = results.find_children(
                lambda node: node.role_name == "label" and "full-value-" in (node.name or "")
            )
            time.sleep(0.1)
        self.assertTrue(cells, dump_tree(results))
        cell = cells[0]
        self.assertNotEqual(cell.name, full_value)
        cell.point()
        wait_for_node(application, full_value, timeout=15)
        wait_for_node(application, "SQL query editor").point()
        wait_for_node_to_disappear(application, full_value)
        self.action(application, "Focus Results", "Focus Results")
        rawinput.press_key("space")
        wait_for_node(application, "Result Row")
        wait_for_node(application, full_value, timeout=15)
        wait_for_node(application, "Close result row", "button").do_action_named("click")
        wait_for_node_to_disappear(application, "Result Row")

        self.run_sql(application, "SELECT 42 AS restarted_value")
        wait_for_node(application, "42", timeout=15)
        wait_for_node_to_disappear(application, "Truncated - 10000 rows (row limit)")

    def test_postgres_streamed_results(self):
        self.check_streamed_results("postgresql")

    def test_clickhouse_streamed_results(self):
        self.check_streamed_results("clickhouse")

    def test_restored_tabs_reconnect_without_running_saved_sql(self):
        (self.data / "dzban").mkdir(parents=True)
        (self.data / "dzban" / "gtk-workspace.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "tabs": [
                        {
                            "profile": "Integration PostgreSQL",
                            "draft": "SELECT 'restored' AS marker",
                        },
                        {"draft": "scratch"},
                    ],
                    "selected": 0,
                }
            ),
            encoding="utf-8",
        )
        application = self.launch(POSTGRES)
        wait_for_node(application, "Integration PostgreSQL - Query 1", timeout=15)
        wait_for_node(application, "New Query")
        editor = wait_for_node(application, "SQL query editor")
        wait_until(
            lambda: editor.text == "SELECT 'restored' AS marker",
            "The restored draft was not shown",
        )
        wait_for_node(application, "Ready to query")
        wait_for_node(application, "Run a query to see results")

    def test_result_cursor_selection_and_freeze(self):
        from dogtail import rawinput

        application = self.launch(POSTGRES)
        wait_for_node(application, "Open", "button")
        rawinput.key_combo("F11")
        wait_until(lambda: application.child(role_name="frame").extents[2:] == self.session.monitor_size(), "The result-grid test window did not fullscreen")
        wait_for_node(application, "Open", "button").do_action_named("click")
        editor = self.run_sql(application, "SELECT i AS id, 'row ' || i AS value FROM generate_series(1, 3) i")
        wait_for_node(application, "row 1")
        self.action(application, "Focus Results", "Focus Results")

        def copied():
            rawinput.key_combo("<Control>c")
            time.sleep(0.2)
            self.action(application, "Focus Query Editor", "Focus Query Editor")
            editor.text = ""
            wait_until(lambda: editor.focused, "The editor did not receive keyboard focus")
            rawinput.key_combo("<Control>v")
            wait_until(lambda: bool(editor.text), "The result selection was not copied")
            return editor.text

        self.assertEqual(copied(), "1")
        self.action(application, "Focus Results", "Focus Results")
        rawinput.key_combo("<Shift>Right")
        self.assertTrue(copied().startswith("| id | value |\n"))
        self.action(application, "Focus Results", "Focus Results")
        rawinput.key_combo("<Control>space")
        self.assertTrue(copied().startswith("| # | id | value |\n"))
        wait_for_node(application, "value").click()
        self.assertEqual(copied(), "| value |\n| --- |\n| row 1 |\n| row 2 |\n| row 3 |\n")
        self.action(application, "Focus Results", "Focus Results")
        rawinput.key_combo("x")
        wait_for_node(application, "Frozen column: value")
        rawinput.key_combo("x")
        wait_for_node_to_disappear(application, "Frozen column: value")


if __name__ == "__main__":
    scenarios = {
        "empty": "test_empty_connections_and_shortcuts_screen",
        "database": "test_query_results_history_and_saved_plan",
        "clickhouse": "test_clickhouse_results_errors_history_and_plan",
        "shutdown": "test_busy_window_close_cancels_work_and_saves_tabs",
        "restore": "test_restored_tabs_reconnect_without_running_saved_sql",
        "streaming-postgres": "test_postgres_streamed_results",
        "streaming-clickhouse": "test_clickhouse_streamed_results",
        "result-grid": "test_result_cursor_selection_and_freeze",
    }
    scenario = os.environ.get("DZBAN_UI_SCENARIO")
    if scenario not in scenarios:
        raise SystemExit(f"DZBAN_UI_SCENARIO must be one of: {', '.join(scenarios)}")
    unittest.main(defaultTest=f"DzbanScreens.{scenarios[scenario]}", verbosity=2)
