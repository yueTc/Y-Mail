-- 0002_accounts_and_proxies：Wave 1 账号与代理表。
-- 关键约束：本表结构里没有任何明文凭据列；账号的授权码、代理的密码都只存
-- 「系统凭据管理器里的引用键」（credential_key / password_key）。

CREATE TABLE IF NOT EXISTS proxy (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    label       TEXT NOT NULL DEFAULT '',
    kind        TEXT NOT NULL CHECK (kind IN ('socks5', 'http')),
    host        TEXT NOT NULL,
    port        INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    username    TEXT NOT NULL DEFAULT '',
    password_key TEXT,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE TABLE IF NOT EXISTS account (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    display_name   TEXT NOT NULL,
    email          TEXT NOT NULL,
    auth_type      TEXT NOT NULL CHECK (auth_type IN ('password', 'oauth2')),
    username       TEXT NOT NULL,
    imap_host      TEXT NOT NULL,
    imap_port      INTEGER NOT NULL CHECK (imap_port BETWEEN 1 AND 65535),
    imap_security  TEXT NOT NULL CHECK (imap_security IN ('tls', 'starttls', 'plain')),
    smtp_host      TEXT NOT NULL,
    smtp_port      INTEGER NOT NULL CHECK (smtp_port BETWEEN 1 AND 65535),
    smtp_security  TEXT NOT NULL CHECK (smtp_security IN ('tls', 'starttls', 'plain')),
    proxy_mode     TEXT NOT NULL DEFAULT 'inherit' CHECK (proxy_mode IN ('inherit', 'direct', 'custom')),
    proxy_id       INTEGER REFERENCES proxy(id) ON DELETE SET NULL,
    color          TEXT NOT NULL DEFAULT '',
    enabled        INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    credential_key TEXT,
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_account_email ON account (email);
CREATE INDEX IF NOT EXISTS idx_account_proxy ON account (proxy_id);

-- 全局代理设置放在 setting 表里：proxy.global_mode = system/direct/custom，proxy.global_id = 代理主键。
INSERT OR IGNORE INTO setting (key, value) VALUES ('proxy.global_mode', 'system');