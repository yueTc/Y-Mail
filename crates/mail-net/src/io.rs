//! 文本行读取小工具：收件与发件协议都是「一行一行聊」的文本协议。

use mail_domain::error::ConnectionError;
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::error::describe_io;

/// 从流里读一行（以 CRLF 结尾），返回不含行尾的内容。
///
/// `max` 限制单行长度，防止对方发超长内容把内存撑爆。
pub async fn read_crlf_line<S>(stream: &mut S, max: usize) -> Result<String, ConnectionError>
where
    S: AsyncRead + Unpin,
{
    let mut buffer = Vec::with_capacity(128);
    loop {
        if buffer.len() > max {
            return Err(ConnectionError::protocol("服务器返回的单行内容过长"));
        }
        let mut byte = [0u8; 1];
        let read = stream
            .read(&mut byte)
            .await
            .map_err(|err| ConnectionError::network(format!("读取服务器响应失败：{}", describe_io(&err))))?;
        if read == 0 {
            return Err(ConnectionError::protocol("服务器没有应答就断开了连接"));
        }
        buffer.push(byte[0]);
        if buffer.ends_with(b"\r\n") {
            let length = buffer.len() - 2;
            buffer.truncate(length);
            return String::from_utf8(buffer)
                .map_err(|_| ConnectionError::protocol("服务器返回的内容不是有效文本"));
        }
    }
}

/// 发一行文本（自动补 CRLF）。
pub async fn write_crlf_line<S>(stream: &mut S, line: &str) -> Result<(), ConnectionError>
where
    S: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;

    stream
        .write_all(line.as_bytes())
        .await
        .map_err(|err| ConnectionError::network(format!("发送请求失败：{}", describe_io(&err))))?;
    stream
        .write_all(b"\r\n")
        .await
        .map_err(|err| ConnectionError::network(format!("发送请求失败：{}", describe_io(&err))))?;
    stream
        .flush()
        .await
        .map_err(|err| ConnectionError::network(format!("发送请求失败：{}", describe_io(&err))))
}
