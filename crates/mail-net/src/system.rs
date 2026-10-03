//! 读取系统代理设置（当前实现针对 Windows 注册表）。
//!
//! 本阶段只认「静态代理服务器」；如果系统只配置了自动配置脚本（PAC），
//! 会明确返回「不支持」，让界面提示用户改用全局自定义代理。

use mail_domain::error::ConnectionError;
use mail_domain::proxy::{ProxyKind, ProxyRoute};

/// 系统代理的读取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemProxy {
    /// 系统没有配置代理。
    None,
    /// 读到了可以直接使用的静态代理。
    Static(ProxyRoute),
    /// 只配置了自动配置脚本，本阶段无法解析。
    AutoConfig {
        /// 自动配置脚本地址。
        url: String,
    },
}

/// 读取当前用户的系统代理设置。
pub fn read_system_proxy() -> Result<SystemProxy, ConnectionError> {
    platform::read()
}

/// 解析 Windows 的「代理服务器」字符串。
///
/// 支持三种写法：
/// - `http=1.2.3.4:8080;https=1.2.3.4:8081`
/// - `socks=1.2.3.4:1080`
/// - `1.2.3.4:8080`（没有协议名时按 HTTP 代理处理）
pub fn parse_proxy_server(raw: &str) -> Option<(ProxyKind, String, u16)> {
    let mut socks: Option<(String, u16)> = None;
    let mut http: Option<(String, u16)> = None;
    let mut plain: Option<(String, u16)> = None;

    for entry in raw
        .split([';', ','])
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        match entry.split_once('=') {
            Some((name, address)) => {
                let name = name.trim().to_ascii_lowercase();
                if let Some(host_port) = split_host_port(address) {
                    match name.as_str() {
                        "socks" | "socks5" => socks = socks.or(Some(host_port)),
                        "http" | "https" => http = http.or(Some(host_port)),
                        _ => {}
                    }
                }
            }
            None => {
                if let Some(host_port) = split_host_port(entry) {
                    let lower = entry.to_ascii_lowercase();
                    if lower.starts_with("socks") {
                        socks = socks.or(Some(host_port));
                    } else {
                        plain = plain.or(Some(host_port));
                    }
                }
            }
        }
    }

    if let Some((host, port)) = socks {
        return Some((ProxyKind::Socks5, host, port));
    }
    if let Some((host, port)) = http {
        return Some((ProxyKind::Http, host, port));
    }
    plain.map(|(host, port)| (ProxyKind::Http, host, port))
}

/// 从 `host:port`（可带协议前缀）里拆出主机与端口。
fn split_host_port(address: &str) -> Option<(String, u16)> {
    let address = address.trim();
    let address = match address.split_once("://") {
        Some((_, rest)) => rest,
        None => address,
    };
    let address = address.split('/').next().unwrap_or(address);
    let (host, port) = address.rsplit_once(':')?;
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    let port: u16 = port.trim().parse().ok()?;
    if port == 0 {
        return None;
    }
    Some((host.to_string(), port))
}

#[cfg(windows)]
mod platform {
    use mail_domain::error::ConnectionError;
    use mail_domain::proxy::{ProxyConfig, ProxyRoute};
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;

    use super::{parse_proxy_server, SystemProxy};

    const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    pub fn read() -> Result<SystemProxy, ConnectionError> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let key = hkcu
            .open_subkey_with_flags(INTERNET_SETTINGS, KEY_READ)
            .map_err(|_| ConnectionError::proxy("无法读取系统代理设置：注册表不可访问"))?;

        let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
        let server: String = key.get_value("ProxyServer").unwrap_or_default();
        let auto_url: String = key.get_value("AutoConfigURL").unwrap_or_default();

        if enabled == 1 {
            if let Some((kind, host, port)) = parse_proxy_server(&server) {
                return Ok(SystemProxy::Static(ProxyRoute {
                    config: ProxyConfig {
                        id: None,
                        label: "系统代理".to_string(),
                        kind,
                        host,
                        port,
                        username: String::new(),
                    },
                    password: None,
                }));
            }
        }

        if !auto_url.trim().is_empty() {
            return Ok(SystemProxy::AutoConfig { url: auto_url });
        }

        Ok(SystemProxy::None)
    }
}

#[cfg(not(windows))]
mod platform {
    use mail_domain::error::ConnectionError;

    use super::SystemProxy;

    /// 非 Windows 系统本阶段暂不读取系统代理，按「没有配置」处理。
    pub fn read() -> Result<SystemProxy, ConnectionError> {
        Ok(SystemProxy::None)
    }
}

#[cfg(test)]
mod tests {
    use mail_domain::proxy::ProxyKind;

    use super::parse_proxy_server;

    #[test]
    fn 解析http与https条目() {
        let parsed = parse_proxy_server("http=127.0.0.1:8080;https=127.0.0.1:8081").expect("应能解析");
        assert_eq!(parsed, (ProxyKind::Http, "127.0.0.1".to_string(), 8080));
    }

    #[test]
    fn 袜子代理优先于http() {
        let parsed = parse_proxy_server("http=127.0.0.1:8080;socks=127.0.0.1:1080").expect("应能解析");
        assert_eq!(parsed, (ProxyKind::Socks5, "127.0.0.1".to_string(), 1080));
    }

    #[test]
    fn 没有协议名时按http处理() {
        let parsed = parse_proxy_server("10.0.0.1:3128").expect("应能解析");
        assert_eq!(parsed, (ProxyKind::Http, "10.0.0.1".to_string(), 3128));
    }

    #[test]
    fn 带协议前缀也能解析() {
        let parsed = parse_proxy_server("http://proxy.example.com:8080/").expect("应能解析");
        assert_eq!(parsed, (ProxyKind::Http, "proxy.example.com".to_string(), 8080));
    }

    #[test]
    fn 无法识别的写法返回空() {
        assert!(parse_proxy_server("").is_none());
        assert!(parse_proxy_server("proxy.example.com").is_none());
        assert!(parse_proxy_server("ftp=127.0.0.1:21").is_none());
    }

    #[test]
    fn 本机读取系统代理不应报错() {
        let _ = super::read_system_proxy().expect("读取系统代理不应报错");
    }
}
