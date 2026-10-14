-- PostgreSQL 15 test database initialization
-- Creates sample tables and data for testing

CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    username VARCHAR(50) NOT NULL UNIQUE,
    email VARCHAR(100) NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE posts (
    id SERIAL PRIMARY KEY,
    user_id INTEGER REFERENCES users(id),
    title VARCHAR(200) NOT NULL,
    content TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Insert test data
INSERT INTO users (username, email) VALUES
    ('alice', 'alice@example.com'),
    ('bob', 'bob@example.com'),
    ('charlie', 'charlie@example.com');

INSERT INTO posts (user_id, title, content) VALUES
    (1, 'First Post', 'Hello from Alice!'),
    (1, 'Second Post', 'Another post by Alice'),
    (2, 'Bob Post', 'Bob''s thoughts'),
    (3, 'Charlie Here', 'Charlie''s first post');

-- Create a view for testing
CREATE VIEW user_post_counts AS
SELECT u.username, COUNT(p.id) as post_count
FROM users u
LEFT JOIN posts p ON u.id = p.user_id
GROUP BY u.id, u.username;
CREATE SCHEMA "catalog odd's schema";
CREATE TABLE "catalog odd's schema"."catalog odd's table" (id integer PRIMARY KEY);
CREATE ROLE catalog_fixture_role NOLOGIN;
CREATE TABLE "catalog žluť[1]\tail" (id integer);
CREATE TABLE "catalog line
break" (id integer);
CREATE TABLE "catalog ?star*" (id integer);
