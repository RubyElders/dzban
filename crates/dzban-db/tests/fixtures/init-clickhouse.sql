CREATE DATABASE IF NOT EXISTS dzban_test;

CREATE TABLE dzban_test.events
(
    id UInt64,
    name String,
    occurred_at DateTime64(3)
)
ENGINE = MergeTree
ORDER BY id;

INSERT INTO dzban_test.events VALUES
    (1, 'connected', '2026-09-24 10:00:00.123'),
    (2, 'queried', '2026-09-24 10:01:00.456'),
    (3, 'rendered', '2026-09-24 10:02:00.789');
