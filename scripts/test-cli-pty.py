import os
import pathlib
import shlex
import sys
import tempfile
import termios
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import signal
import subprocess

import pexpect


ROOT = pathlib.Path(__file__).resolve().parents[1]
CONFIG = ROOT / "crates/dzban-cli/tests/fixtures/config.toml"
BINARY = ROOT / "target/debug/dzb"


class Console(pexpect.spawn):
    def send(self, text):
        return super().send(text.replace("\r\\", "\x1b[13;2u\\"))

    def send_wire(self, text):
        return super().send(text)


def launch(data, *arguments, editor_command=None, config=None):
    environment = dict(os.environ, XDG_DATA_HOME=data, TERM="xterm-256color")
    if editor_command is not None:
        environment["DZBAN_EDITOR"] = editor_command
    return Console(
        str(BINARY),
        ["--config", str(config or CONFIG), *arguments],
        env=environment,
        encoding="utf-8",
        timeout=15,
        dimensions=(30, 100),
    )


def finish(child, code=0):
    child.expect(pexpect.EOF)
    attributes = termios.tcgetattr(child.child_fd)
    assert attributes[3] & termios.ICANON
    assert attributes[3] & termios.ECHO
    child.close()
    assert child.exitstatus == code, (child.exitstatus, child.signalstatus)


with tempfile.TemporaryDirectory(prefix="dzb-pty-") as data:
    for profile, table in [("pg", "users"), ("ch", "events")]:
        basic = launch(data, "--connection", profile, "c", "--no-pager")
        basic.expect("Connected to " + profile)
        basic.send_wire("SELECT 101 AS plain_enter\r")
        try:
            basic.expect(r"\(1 rows\)", timeout=0.5)
            raise AssertionError("Enter submitted SQL without a terminator")
        except pexpect.TIMEOUT:
            pass
        basic.send_wire(";\r")
        basic.expect(r"\| 101\s+\|")
        basic.expect(r"\(1 rows\)")
        basic.send_wire("SELECT 102 AS control_enter;\x1b[13;5u")
        basic.expect(r"\| 102\s+\|")
        basic.expect(r"\(1 rows\)")
        basic.send_wire("SELECT\x1b[13;2u103 AS multiline;\r")
        basic.expect(r"\| 103\s+\|")
        basic.expect(r"\(1 rows\)")
        basic.send_wire("SELECT 104 AS preserved_draft")
        basic.expect("preserved_draft")
        basic.send_wire("\x0c")
        basic.expect_exact("\x1b[2J\x1b[H")
        basic.expect("preserved_draft")
        basic.send_wire("\x1b[A")
        basic.expect("103 AS multiline;")
        basic.send_wire("\x1b[B")
        basic.expect("104 AS preserved_draft")
        basic.send_wire(";\r")
        basic.expect(r"\| 104\s+\|")
        basic.expect(r"\(1 rows\)")
        basic.send_wire("\x10")
        basic.expect("104 AS preserved_draft;")
        basic.send_wire("\x0e")
        basic.send_wire("SELECT 999\x17")
        basic.expect(r"> SELECT ")
        basic.send_wire("105 AS edited;\x01\x0b\x19\r")
        basic.expect(r"\| 105\s+\|")
        basic.expect(r"\(1 rows\)")
        basic.send_wire("\\d\r")
        basic.expect("Schema" if profile == "pg" else "Database")
        basic.expect(table)
        basic.expect(r"\(\d+ rows\)")
        basic.send_wire("\\dt\r")
        basic.expect(table)
        basic.expect(r"\(\d+ rows\)")
        basic.send_wire("\\l\r")
        basic.expect("testdb" if profile == "pg" else "dzban_test")
        basic.expect(r"\(\d+ rows\)")
        basic.send_wire("\\du\r")
        basic.expect("testuser" if profile == "pg" else "dzban")
        basic.expect(r"\(\d+ rows\)")
        basic.send_wire("\\dg catalog_fixture_role\r")
        basic.expect(r"\| catalog_fixture_role\s+\|")
        basic.expect(r"\(1 rows\)")
        basic.send_wire("\\di\r")
        basic.expect("PostgreSQL index" if profile == "pg" else "ClickHouse data-skipping index")
        basic.expect(r"\(\d+ rows\)")
        basic.send_wire("\\dt 'catalog odd*'\r")
        basic.expect("catalog odd's table")
        basic.expect(r"\(1 rows\)")
        basic.send_wire("\\dnS pg_catalog\r" if profile == "pg" else "\\dn\r")
        basic.expect("pg_catalog" if profile == "pg" else "Unsupported catalog action")
        if profile == "pg":
            basic.expect(r"\(1 rows\)")
        basic.send_wire("\\d " + table + "\r")
        basic.expect("column_name")
        basic.expect("integer" if profile == "pg" else "UInt64")
        basic.expect(r"\(4 rows\)" if profile == "pg" else r"\(3 rows\)")
        basic.send_wire("\\d+ 'catalog odd'\"'\"'s table'\r")
        basic.expect("comment")
        basic.expect(r"\(1 rows\)")
        basic.send_wire("\\d dzban_missing_object\r")
        basic.expect("No matching object")
        basic.send_wire("SELECT unfinished\x1b[13;2u  \\q  \x1b[13;5u")
        finish(basic)
        reopened = launch(data, "--connection", profile, "c", "--no-pager")
        reopened.expect("Connected to " + profile)
        reopened.send_wire("\x1b[A")
        reopened.expect("105 AS edited;")
        reopened.send_wire("\x03\\q\r")
        finish(reopened)

    picker = launch(data, "c", "--no-pager")
    picker.expect("Select connection")
    picker.send("missing")
    picker.expect("No matching items")
    picker.send("\x1b")
    finish(picker, 130)

    console = launch(data, "c", "--no-pager")
    console.expect("Select connection")
    console.send("pg\r")
    console.expect("Connected to pg")
    console.send("\\?\r")
    console.expect_exact("Query\r\n")
    console.expect_exact("Explain and execute the submitted SQL")
    console.expect_exact("Cancel the specified active query")
    console.expect_exact("Output and history")
    console.send("\\cancel\r")
    console.expect_exact("No query is running")
    console.send("\\conninfo\r")
    console.expect_exact("pg | Postgresql | database: testdb | user: testuser")
    console.send("\\connections\r")
    console.expect_exact("ch | Clickhouse")
    console.send("\\c missing-profile\r")
    console.expect("Unknown connection profile: missing-profile")
    console.send("\\conninfo\r")
    console.expect_exact("pg | Postgresql | database: testdb")
    console.send("\\connect ch\r")
    console.expect("Connected to ch")
    console.send("SELECT 314 AS switched_value\r\\g\r")
    console.expect(r"\| 314\s+\|")
    console.expect(r"\(1 rows\)")
    console.send("\\c pg\r")
    console.expect("Connected to pg")
    console.send("\\timing on\r")
    console.expect("Timing on")
    for sql, affected in [
        ("BEGIN", 0),
        ("CREATE TEMP TABLE dzb_ack (value integer)", 0),
        ("INSERT INTO dzb_ack VALUES (1), (2)", 2),
        ("UPDATE dzb_ack SET value = 3 WHERE value = 1", 1),
        ("ROLLBACK", 0),
    ]:
        console.send(sql + "\r\\g\r")
        console.expect_exact(f"Command completed ({affected} affected rows)")
        console.expect(r"Time: [0-9.]+ ms total \| Planning: [0-9.]+ ms")
    console.send("\\pager\r")
    console.expect("Pager off")
    console.send("\\pager on extra\r")
    console.expect(r"Usage: \\pager \[on\|off\]")
    console.send("\\pager on\r")
    console.expect("Pager on")
    console.send("\\pager off\r")
    console.expect("Pager off")
    console.send("SELECT 4\x1b[D2\x1b[F\r\\g\r")
    console.expect(r"\| 24\s+\|")
    console.expect(r"\(1 rows\)")
    console.send("SELECT 45 AS expanded_once\r\\gx\r")
    console.expect("expanded_once: 45")
    console.send("SELECT 46 AS table_again\r\\g\r")
    console.expect(r"\| 46\s+\|")
    console.expect(r"\(1 rows\)")
    console.send("SELECT 42 AS answer\r\\g\r")
    console.expect("answer")
    console.expect(r"\(1 rows\)")
    console.expect(r"Time: [0-9.]+ ms total \| Planning: [0-9.]+ ms \| Execution: [0-9.]+ ms")
    console.send("\\plan\r")
    console.expect('"document"')
    console.send("\\s\r")
    console.expect("SELECT 42 AS answer")
    console.send("\x12")
    console.expect("History search")
    console.send("42 AS answer\r")
    console.expect(r"> SELECT 42 AS answer")
    console.send("\r\\g\r")
    console.expect(r"\| 42\s+\|")
    console.expect(r"\(1 rows\)")
    console.send("SELECT 99")
    console.send("\x12")
    console.expect("History search")
    console.send("missing query")
    console.expect("No matching items")
    console.send("\x1b")
    console.expect(r"> SELECT 99")
    console.send("\x03")
    console.send("SELECT pg_sleep(10)\r\\g\r")
    console.expect(r"\[running [0-9.]+s\]")
    console.send("\\c ch\r")
    console.expect("Cancel the running query before switching connections")
    console.send("\x12")
    console.expect("History search")
    console.send("\x1b")
    console.expect("Query cancelled")
    console.expect(r"Time: [0-9.]+ ms total \| Planning: [0-9.]+ ms")
    console.send("SELECT 43 AS answer\r\\g\r")
    console.expect(r"\(1 rows\)")
    console.expect(r"Time: [0-9.]+ ms total \| Planning: [0-9.]+ ms")
    console.send("SELECT dzb_missing_column\r\\g\r")
    console.expect('column "dzb_missing_column" does not exist')
    console.expect(r"Time: [0-9.]+ ms total \| Planning: [0-9.]+ ms")
    console.send("\\errverbose\r")
    console.expect('"SqlState": "42703"')
    console.send("SELECT pg_sleep(10)\r\\g\r")
    console.expect(r"\[running [0-9.]+s\]")
    console.send("\x03")
    console.expect("Query cancelled")
    console.expect(r"Time: [0-9.]+ ms total \| Planning: [0-9.]+ ms")
    console.send("\\timing\r")
    console.expect("Timing off")
    console.send("SELECT pg_sleep(10)\r\\g\r")
    console.expect(r"\[running [0-9.]+s\]")
    console.send("\\cancel\r")
    console.expect("Query cancelled")
    console.send("SELECT 44 AS after_command_cancel\r\\g\r")
    console.expect(r"\| 44\s+\|")
    console.expect(r"\(1 rows\)")
    console.setwinsize(20, 75)
    console.send("\\q\r")
    finish(console)

    editor_command = shlex.join([
        sys.executable,
        "-c",
        'import pathlib, sys, termios; flags = termios.tcgetattr(0)[3]; '
        'assert flags & termios.ICANON and flags & termios.ECHO; '
        'source = pathlib.Path(sys.argv[1]).read_text(); '
        'pathlib.Path(sys.argv[1]).write_text("SELECT 77 AS edited_value;\\n"); '
        'print("Editor input was draft" if "draft_value" in source else "Editor input was submitted SQL"); '
        'print("Editor terminal ready")',
    ])
    edited = launch(data, "--connection", "pg", "c", "--no-pager", editor_command=editor_command)
    edited.expect("Connected to pg")
    edited.send("SELECT 76 AS original_value\r\\g\r")
    edited.expect(r"\| 76\s+\|")
    edited.expect(r"\(1 rows\)")
    edited.send("\\e\r")
    edited.expect("Editor terminal ready")
    edited.expect(r"> SELECT 77 AS edited_value;")
    edited.send("\\g\r")
    edited.expect(r"\| 77\s+\|")
    edited.expect(r"\(1 rows\)")
    saved_query = pathlib.Path(data) / "saved query.sql"
    edited.send("SELECT 66 AS draft_value\r\\w " + shlex.quote(str(saved_query)) + "\r")
    edited.expect("Saved query buffer")
    edited.expect(r"> SELECT 66 AS draft_value")
    assert saved_query.read_text() == "SELECT 66 AS draft_value\n"
    edited.send("\\write " + shlex.quote(str(saved_query)) + "\r")
    edited.expect("Cannot create query file")
    edited.expect(r"> SELECT 66 AS draft_value")
    assert saved_query.read_text() == "SELECT 66 AS draft_value\n"
    edited.send("\\p\r")
    edited.expect(r"> SELECT 66 AS draft_value")
    edited.send("\\e\r")
    edited.expect("Editor input was draft")
    edited.expect("Editor terminal ready")
    edited.expect(r"> SELECT 77 AS edited_value;")
    edited.send("\\g\r")
    edited.expect(r"\| 77\s+\|")
    edited.expect(r"\(1 rows\)")
    edited.send("\\q\r")
    finish(edited)

    editor_failure = launch(data, "--connection", "pg", "c", "--no-pager", editor_command="/bin/false")
    editor_failure.expect("Connected to pg")
    editor_failure.send("SELECT 88 AS preserved_sql\r\\g\r")
    editor_failure.expect(r"\(1 rows\)")
    editor_failure.send("\\edit\r")
    editor_failure.expect("keeping original SQL")
    editor_failure.expect(r"> SELECT 88 AS preserved_sql")
    editor_failure.send("\x03\\q\r")
    finish(editor_failure)

    for profile, sql in [
        ("pg", "SELECT i AS live_row, repeat('x',9000), pg_sleep(0.1) FROM generate_series(1,100) i"),
        ("ch", "SELECT number + 1 AS live_row, repeat('x',9000), sleepEachRow(0.1) FROM numbers(100) SETTINGS max_block_size=1, max_threads=1"),
    ]:
        live = launch(data, "--connection", profile, "c", "--no-pager")
        live.expect(f"Connected to {profile}")
        live.send(sql + "\r\\g\r")
        live.expect("Row 1: live_row: 1")
        live.send("\x03")
        live.expect("Query cancelled")
        live.send("SELECT 55 AS restarted_live\r\\g\r")
        live.expect(r"\| 55\s+\|")
        live.expect(r"\(1 rows\)")
        live.send("\\q\r")
        finish(live)

    paged = launch(data, "--connection", "pg", "c", "--pager", "pspg")
    paged.expect("Connected to pg")
    paged.send("SELECT generate_series(1,100) AS pager_value\r\\g\r")
    paged.expect(r"\+-{5,}")
    paged.send("q")
    paged.expect(r"\[ready\]")
    paged.send("\\q\r")
    finish(paged)

    plan_pager = launch(
        data, "--connection", "pg", "c", "--pager",
        "/usr/bin/python3 -c 'import sys,time; sys.stdin.read(); time.sleep(3)'",
    )
    plan_pager.expect("Connected to pg")
    plan_pager.send("\\timing on\r")
    plan_pager.expect("Timing on")
    plan_pager.send("SELECT pg_sleep(1)\r\\g\r")
    plan_pager.expect(r"\[running")
    plan_pager.send("\\plan\r")
    plan_pager.expect(r"Time: ([0-9.]+) ms total")
    assert float(plan_pager.match.group(1)) < 2500, "Plan viewing delayed core query completion"
    plan_pager.expect(r"\[ready\]")
    plan_pager.send("\\q\r")
    finish(plan_pager)

    fallback = launch(data, "--connection", "pg", "c", "--pager", "/nonexistent-dzb-test-pager")
    fallback.expect("Connected to pg")
    fallback.send("\x1b[200~SELECT 'pasted value' AS value\n\x1b[201~")
    fallback.send("\\g\r")
    fallback.expect("displaying without pager")
    fallback.expect(r"\(1 rows\)")
    fallback.send("\\q\r")
    finish(fallback)

    query_started = threading.Event()
    query_finished = threading.Event()
    cancellation_started = threading.Event()
    cancellation_released = threading.Event()

    class CancelFailureFixture(BaseHTTPRequestHandler):
        def log_message(self, *arguments):
            pass

        def do_POST(self):
            sql = self.rfile.read(int(self.headers["Content-Length"])).decode()
            if sql.startswith("KILL QUERY"):
                cancellation_started.set()
                if not cancellation_released.wait(10):
                    self.send_error(500, "Fixture cancellation wait timed out")
                    return
                self.send_response(500)
                self.send_header("X-ClickHouse-Exception-Code", "497")
                self.end_headers()
                self.wfile.write(b"Fixture cancellation denied")
                query_finished.set()
                return
            if sql.startswith("EXPLAIN ESTIMATE"):
                self.send_response(500)
                self.send_header("X-ClickHouse-Exception-Code", "497")
                self.end_headers()
                self.wfile.write(b"Fixture estimate unavailable")
                return
            elif sql.startswith("EXPLAIN"):
                body = '["plan"]\n["String"]\n' + json.dumps(["{}"]) + "\n"
            elif sql == "SELECT version()":
                body = '["version"]\n["String"]\n["Fixture 1.0"]\n'
            else:
                if "fixture_wait" in sql:
                    query_started.set()
                    if not query_finished.wait(10):
                        self.send_error(500, "Fixture wait timed out")
                        return
                value = 54 if "54" in sql else 53
                body = '["value"]\n["UInt64"]\n' + json.dumps([value]) + "\n"
            self.send_response(200)
            self.end_headers()
            self.wfile.write(body.encode())

    server = ThreadingHTTPServer(("127.0.0.1", 0), CancelFailureFixture)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        fixture_config = pathlib.Path(data) / "cancel-fixture.toml"
        fixture_config.write_text(
            '[[databases]]\nname = "fixture"\nbackend = "clickhouse"\n'
            f'url = "http://127.0.0.1:{server.server_port}"\n'
            'user = "fixture"\ndatabase = "fixture"\n'
        )
        for cancellation in ["\x03", "\\cancel\r"]:
            query_started.clear()
            query_finished.clear()
            cancellation_started.clear()
            cancellation_released.clear()
            failed_cancel = launch(data, "--connection", "fixture", "c", "--no-pager", config=fixture_config)
            failed_cancel.expect("Connected to fixture")
            failed_cancel.send("SELECT 53 AS fixture_wait\r\\g\r")
            assert query_started.wait(5)
            failed_cancel.send(cancellation)
            assert cancellation_started.wait(5)
            failed_cancel.send("SELECT 54")
            failed_cancel.expect(r"> SELECT 54")
            cancellation_released.set()
            failed_cancel.expect("Fixture cancellation denied")
            failed_cancel.expect(r"\| 53\s+\|")
            failed_cancel.expect(r"\(1 rows\)")
            failed_cancel.send("\x03\\plan\r")
            failed_cancel.expect("Fixture estimate unavailable")
            failed_cancel.expect(r'"ClickHouse": 497')
            failed_cancel.send("\\errverbose\r")
            failed_cancel.expect(r'"ClickHouse": 497')
            failed_cancel.send("SELECT 54\r\\g\r")
            failed_cancel.expect(r"\| 54\s+\|")
            failed_cancel.expect(r"\(1 rows\)")
            failed_cancel.send("\\q\r")
            finish(failed_cancel)
        query_started.clear()
        query_finished.clear()
        cancellation_started.clear()
        cancellation_released.clear()
        batch_sql = "SELECT 53 AS fixture_wait_batch"
        batch = subprocess.Popen(
            [str(BINARY), "--config", str(fixture_config), "--connection", "fixture", "q", batch_sql],
            env=dict(os.environ, XDG_DATA_HOME=data),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            assert query_started.wait(5)
            batch.send_signal(signal.SIGINT)
            assert cancellation_started.wait(5)
            assert batch.poll() is None
            cancellation_released.set()
            stdout, stderr = batch.communicate(timeout=10)
            assert batch.returncode == 0, (batch.returncode, stderr)
            assert "53" in stdout and "(1 rows)" in stdout, stdout
            assert "cancellation failed" in stderr and "Fixture cancellation denied" in stderr, stderr
            records = [json.loads(path.read_text()) for path in pathlib.Path(data).rglob("*.json")]
            assert any(record.get("entry", {}).get("sql") == batch_sql for record in records), records
        finally:
            if batch.poll() is None:
                batch.kill()
                batch.wait()
    finally:
        cancellation_released.set()
        query_finished.set()
        server.shutdown()
        server.server_close()
        worker.join()

print("CLI PTY picker, query, plan, history, cancel, restart, resize, paste, pager, and terminal cleanup passed")
