CREATE TABLE IF NOT EXISTS native_chat_configuration(order_id TEXT PRIMARY KEY REFERENCES orders(id),revision BIGINT NOT NULL,payload TEXT NOT NULL);
