CREATE DATABASE IF NOT EXISTS dzban_test;

CREATE ROLE catalog_fixture_role;
CREATE USER catalog_limited IDENTIFIED BY 'catalog-fixture-password';
CREATE TABLE dzban_test.`catalog odd's table` (id UInt64) ENGINE = Memory;

CREATE TABLE dzban_test.events
(
    id UInt64,
    name String,
    occurred_at DateTime64(3),
    INDEX name_index name TYPE bloom_filter GRANULARITY 1
)
ENGINE = MergeTree
ORDER BY id;

INSERT INTO dzban_test.events VALUES
    (1, 'connected', '2026-09-24 10:00:00.123'),
    (2, 'queried', '2026-09-24 10:01:00.456'),
    (3, 'rendered', '2026-09-24 10:02:00.789');

CREATE VIEW dzban_test.event_names AS SELECT id, name FROM dzban_test.events;
CREATE TABLE dzban_test.`catalog žluť[1]\\tail` (id UInt64) ENGINE = Memory;
CREATE TABLE dzban_test.`catalog line\nbreak` (id UInt64) ENGINE = Memory;
CREATE TABLE dzban_test.`catalog ?star*` (id UInt64) ENGINE = Memory;
