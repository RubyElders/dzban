-- PostgreSQL 16 production-like database
-- Larger dataset for performance testing

CREATE TABLE products (
    id SERIAL PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    price DECIMAL(10, 2) NOT NULL,
    stock INTEGER DEFAULT 0,
    category VARCHAR(50)
);

CREATE TABLE orders (
    id SERIAL PRIMARY KEY,
    product_id INTEGER REFERENCES products(id),
    quantity INTEGER NOT NULL,
    order_date TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Insert larger dataset
INSERT INTO products (name, price, stock, category)
SELECT
    'Product ' || generate_series,
    (random() * 1000)::decimal(10,2),
    (random() * 100)::int,
    CASE (random() * 4)::int
        WHEN 0 THEN 'Electronics'
        WHEN 1 THEN 'Clothing'
        WHEN 2 THEN 'Food'
        ELSE 'Other'
    END
FROM generate_series(1, 1000);

-- Create indexes
CREATE INDEX idx_products_category ON products(category);
CREATE INDEX idx_orders_product_id ON orders(product_id);
