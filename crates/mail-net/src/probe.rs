//! 网站连通性探测：隧道建好后，真发一次请求，用来量真实延迟。
//!
//! 为什么需要：有些代理会「乐观应答」，还没真连上目标就先回「成功」，
//! 结果只测隧道建立会量到 1 毫秒左右的假数字。必须让目标服务器真的回话。

use std::time::Duration;

use mail_domain::error::ConnectionError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::stream::Stream;

/// 在已建好的连接上做一次真实往返，确认目标网站真的回话了。
///
/// - 目标端口 443：先做 TLS 握手，再发一个 `HEAD` 请求。
/// - 目标端口 80：直接发 `HEAD` 请求。
/// - 其它端口：不额外发请求，直接返回（这类目标只确认隧道能建立）。
pub async fn probe_website(
    stream: Stream,
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<(), ConnectionError> {
    if port != 80 && port != 443 {
        return Ok(());
    }
    match tokio::time::timeout(timeout, probe_http(stream, host, port)).await {
        Ok(result) => result,
        Err(_) => Err(ConnectionError::timeout("连接超时：目标网站在限定时间内没有回话")),
    }
}

async fn probe_http(stream: Stream, host: &str, port: u16) -> Result<(), ConnectionError> {
    let mut stream = if port == 443 {
        stream.wrap_tls(host).await?
    } else {
        stream
    };

    let request = format!(
        "HEAD / HTTP/1.1\r\nHost: {host}\r\nUser-Agent: ymail-proxy-test\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.map_err(|err| {
        ConnectionError::network(format!("发送测试请求失败：{}", crate::error::describe_io(&err)))
    })?;
    stream.flush().await.map_err(|err| {
        ConnectionError::network(format!("发送测试请求失败：{}", crate::error::describe_io(&err)))
    })?;

    read_http_head(&mut stream).await
}

/// 读到 HTTP 应答头（到空行为止），确认服务器真的回了 HTTP 应答。
async fn read_http_head<S>(stream: &mut S) -> Result<(), ConnectionError>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = Vec::with_capacity(256);
    loop {
        if buffer.len() >= 8192 {
            return Err(ConnectionError::protocol("网站返回的应答头过长"));
        }
        let mut byte = [0u8; 1];
        let read = stream.read(&mut byte).await.map_err(|err| {
            ConnectionError::network(format!("读取测试应答失败：{}", crate::error::describe_io(&err)))
        })?;
        if read == 0 {
            return Err(ConnectionError::network(
                "网站在没有返回任何应答的情况下关闭了连接",
            ));
        }
        buffer.push(byte[0]);
        if buffer.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    if !buffer.starts_with(b"HTTP/") {
        return Err(ConnectionError::protocol("网站返回的内容不是有效的 HTTP 应答"));
    }
    Ok(())
}
