//! 连接流：明文 TCP 与 TLS 加密连接的统一封装。

use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

use mail_domain::error::ConnectionError;
use rustls::pki_types::ServerName;
use rustls::ClientConfig;
use rustls_platform_verifier::ConfigVerifierExt;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

/// 系统证书校验配置只需建立一次；连接失败时也复用同一次初始化结果。
static TLS_CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();

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

/// 把一条明文 TCP 连接升级为 TLS 加密连接。
///
/// 加密握手由 Rustls 完成，证书和域名仍交给操作系统信任库校验。
pub async fn tls_wrap(tcp: TcpStream, host: &str) -> Result<Stream, ConnectionError> {
    let config = tls_config()?;
    let server_name = tls_server_name(host)?;
    let tls = TlsConnector::from(config)
        .connect(server_name, tcp)
        .await
        .map_err(|err| {
            tracing::debug!(error = %err, "TLS 握手失败");
            crate::error::classify_tls(&err)
        })?;
    Ok(Stream::Tls(Box::pin(tls)))
}

/// 初始化并缓存 Rustls 平台证书校验器。
fn tls_config() -> Result<Arc<ClientConfig>, ConnectionError> {
    let cached = TLS_CONFIG.get_or_init(|| {
        ClientConfig::with_platform_verifier()
            .map(Arc::new)
            .map_err(|err| err.to_string())
    });
    match cached {
        Ok(config) => Ok(config.clone()),
        Err(error) => {
            tracing::debug!(error = %error, "创建 TLS 连接器失败");
            Err(ConnectionError::tls("无法初始化加密组件"))
        }
    }
}

/// 把服务器地址转换成证书必须覆盖的域名。
fn tls_server_name(host: &str) -> Result<ServerName<'static>, ConnectionError> {
    ServerName::try_from(host.to_owned()).map_err(|error| {
        tracing::debug!(error = %error, "服务器地址不能作为 TLS 域名");
        ConnectionError::tls("服务器地址不能作为证书域名")
    })
}

#[cfg(test)]
mod tests {
    use super::tls_server_name;

    #[test]
    fn 空服务器地址不能用于证书校验() {
        let error = tls_server_name("").expect_err("空地址应被拒绝");
        assert_eq!(error.message, "服务器地址不能作为证书域名");
    }

    #[test]
    fn 正常邮箱服务器地址可用于证书校验() {
        assert!(tls_server_name("smtp.gmail.com").is_ok());
    }
}
