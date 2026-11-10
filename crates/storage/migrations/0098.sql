CREATE TABLE IF NOT EXISTS native_chat_retirements(order_id TEXT PRIMARY KEY REFERENCES orders(id),revision BIGINT NOT NULL,retired_at BIGINT NOT NULL);
