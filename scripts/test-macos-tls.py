#!/usr/bin/env python3
import json
import os
from pathlib import Path
import subprocess
import tempfile
import uuid


def run(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def main():
    if os.uname().sysname != "Darwin":
        raise SystemExit("Run this gate on macOS to exercise its native TLS adapters")
    repo = Path(__file__).resolve().parent.parent
    project = "dzban-tls-" + uuid.uuid4().hex[:12]
    (repo / "target").mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="dzban-tls-", dir=repo / "target") as directory:
        root = Path(directory)
        root.chmod(0o755)
        for name in ("ca", "untrusted"):
            run("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                "-subj", f"/CN=Dzban disposable {name}", "-keyout", str(root / f"{name}.key"),
                "-out", str(root / f"{name}.crt"), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        run("openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=localhost",
            "-keyout", str(root / "server.key"), "-out", str(root / "server.csr"),
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        (root / "extensions").write_text("subjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\nbasicConstraints=CA:FALSE\n")
        run("openssl", "x509", "-req", "-in", str(root / "server.csr"), "-CA", str(root / "ca.crt"),
            "-CAkey", str(root / "ca.key"), "-CAcreateserial", "-days", "1", "-extfile", str(root / "extensions"),
            "-out", str(root / "server.crt"), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        (root / "clickhouse.xml").write_text("""<clickhouse>
<https_port>8443</https_port><http_port remove="1"/>
<openSSL><server><certificateFile>/tls/server.crt</certificateFile>
<privateKeyFile>/tls/server.key</privateKeyFile></server></openSSL>
</clickhouse>
""")
        for path in root.iterdir():
            if path.is_file():
                path.chmod(0o644)
        compose = {
            "services": {
                "postgres": {
                    "image": "postgres:17-alpine", "user": "root",
                    "environment": {"POSTGRES_USER": "dzban", "POSTGRES_PASSWORD": "disposable-tls-fixture", "POSTGRES_DB": "default"},
                    "ports": ["127.0.0.1::5432"], "volumes": [f"{root}:/tls:ro"],
                    "entrypoint": ["/bin/sh", "-ec", "cp /tls/server.key /tmp/server.key; chown postgres:postgres /tmp/server.key; chmod 600 /tmp/server.key; exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/tls/server.crt -c ssl_key_file=/tmp/server.key"],
                    "healthcheck": {"test": ["CMD-SHELL", "pg_isready -U dzban -d default"], "interval": "2s", "timeout": "3s", "retries": 30},
                },
                "clickhouse": {
                    "image": "clickhouse/clickhouse-server:25.8-alpine",
                    "environment": {"CLICKHOUSE_USER": "dzban", "CLICKHOUSE_PASSWORD": "disposable-tls-fixture", "CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT": "1"},
                    "ports": ["127.0.0.1::8443"],
                    "volumes": [f"{root}:/tls:ro", f"{root / 'clickhouse.xml'}:/etc/clickhouse-server/config.d/tls.xml:ro"],
                    "healthcheck": {"test": ["CMD-SHELL", "clickhouse-client --user dzban --password disposable-tls-fixture --query 'SELECT 1'"], "interval": "2s", "timeout": "3s", "retries": 30},
                },
            }
        }
        specification = root / "compose.json"
        specification.write_text(json.dumps(compose))
        command = ["docker", "compose", "-p", project, "-f", str(specification)]
        try:
            run(*command, "up", "-d", "--wait", cwd=repo)
            environment = os.environ.copy()
            for variable in ("SSL_CERT_FILE", "SSL_CERT_DIR", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"):
                environment.pop(variable, None)
            environment["NO_PROXY"] = "localhost,127.0.0.1"
            environment["CCACHE_DISABLE"] = "1"
            for service, port, variable in (("postgres", "5432", "DZBAN_TLS_PG_PORT"), ("clickhouse", "8443", "DZBAN_TLS_CH_PORT")):
                address = run(*command, "port", service, port, capture_output=True).stdout.strip()
                environment[variable] = address.rsplit(":", 1)[1]
            for mode, certificate in (("system", None), ("untrusted", "untrusted.crt"), ("hostname", "ca.crt"), ("trusted", "ca.crt")):
                environment["DZBAN_TLS_EXPECT"] = mode
                if certificate:
                    environment["SSL_CERT_FILE"] = str(root / certificate)
                else:
                    environment.pop("SSL_CERT_FILE", None)
                print(f"TLS gate: {mode}", flush=True)
                run("cargo", "+1.98.1", "test", "--locked", "-p", "dzban-db", "--test", "tls_endpoints", "--", "--ignored", "--test-threads=1", cwd=repo, env=environment)
        except BaseException:
            subprocess.run([*command, "ps"], check=False)
            subprocess.run([*command, "logs", "--tail", "40"], check=False)
            raise
        finally:
            run(*command, "down", "--volumes", "--remove-orphans", cwd=repo)
    print("TLS gates passed; disposable services and certificates removed. Trusted success uses SSL_CERT_FILE; default macOS roots reject the private CA.")


if __name__ == "__main__":
    main()
