//! 连接流：明文 TCP 与 TLS 加密连接的统一封装。

use std::pin::Pin;
use std::task::{Context, Poll};

use mail_domain::error::ConnectionError;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_native_tls::TlsStream;

/// 一条可读可写的连接：要么是明文，要么是 TLS 加密。
///
/// TLS 用 `Pin<Box<..>>` 包住，保证整体可以安全地用 `&mut` 访问，不依赖 unsafe。
pub enum Stream {
    /// 明文 TCP。
    Plain(TcpStream),
    /// TLS 加密后的 TCP。
    Tls(Pin<Box<TlsStream<TcpStream>>>),
}

impl Stream {
    /// 包一条明文连接。
    pub fn plain(tcp: TcpStream) -> Self {
        Self::Plain(tcp)
    }

    /// 是否是明文连接（STARTTLS 只能在明文连接上做）。
    pub fn is_plain(&self) -> bool {
        matches!(self, Self::Plain(_))
    }

    /// 给这条连接套上 TLS：隐式加密直接用，STARTTLS 在升级时用。
    pub async fn wrap_tls(self, host: &str) -> Result<Self, ConnectionError> {
        match self {
            Self::Plain(tcp) => tls_wrap(tcp, host).await,
            Self::Tls(_) => Err(ConnectionError::protocol("连接已经加密，不需要重复升级")),
        }
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.as_mut().get_mut() {
            Self::Plain(tcp) => Pin::new(tcp).poll_read(cx, buf),
            Self::Tls(tls) => tls.as_mut().poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.as_mut().get_mut() {
            Self::Plain(tcp) => Pin::new(tcp).poll_write(cx, buf),
            Self::Tls(tls) => tls.as_mut().poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.as_mut().get_mut() {
            Self::Plain(tcp) => Pin::new(tcp).poll_flush(cx),
            Self::Tls(tls) => tls.as_mut().poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.as_mut().get_mut() {
            Self::Plain(tcp) => Pin::new(tcp).poll_shutdown(cx),
            Self::Tls(tls) => tls.as_mut().poll_shutdown(cx),
        }
    }
}

/// 把一条明文 TCP 连接升级为 TLS 加密连接（使用系统信任库校验证书）。
pub async fn tls_wrap(tcp: TcpStream, host: &str) -> Result<Stream, ConnectionError> {
    let connector = native_tls::TlsConnector::builder().build().map_err(|err| {
        tracing::debug!(error = %err, "创建 TLS 连接器失败");
        ConnectionError::tls("无法初始化加密组件")
    })?;
    let connector = tokio_native_tls::TlsConnector::from(connector);
    let tls = connector.connect(host, tcp).await.map_err(|err| {
        tracing::debug!(error = %err, "TLS 握手失败");
        ConnectionError::tls("加密握手失败：请确认服务器地址与加密方式是否匹配")
    })?;
    Ok(Stream::Tls(Box::pin(tls)))
}
