-- 0007_oauth_accounts：Wave 6 OAuth2 账号字段。
-- 说明：表里只加「服务商标识」与「客户端编号」两列，这两样都不是密钥。
-- 刷新令牌、访问令牌一律只进系统凭据库（keyring），绝不落库、绝不进日志。
-- 密码登录账号这两列为 NULL / 空串，不影响既有数据。

ALTER TABLE account ADD COLUMN oauth_provider TEXT;
ALTER TABLE account ADD COLUMN oauth_client_id TEXT NOT NULL DEFAULT '';