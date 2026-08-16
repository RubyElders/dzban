-- PostgreSQL 17 development database
-- Simple schema for basic testing

CREATE TABLE tasks (
    id SERIAL PRIMARY KEY,
    title VARCHAR(200) NOT NULL,
    completed BOOLEAN DEFAULT FALSE,
    priority INTEGER DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO tasks (title, completed, priority) VALUES
    ('Test task 1', false, 1),
    ('Test task 2', true, 2),
    ('Test task 3', false, 3);

-- Create a function for testing
CREATE OR REPLACE FUNCTION get_incomplete_tasks()
RETURNS TABLE(id INT, title VARCHAR, priority INT) AS $$
BEGIN
    RETURN QUERY SELECT t.id, t.title, t.priority FROM tasks t WHERE NOT t.completed;
END;
$$ LANGUAGE plpgsql;
