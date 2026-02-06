use std::net::IpAddr;

use reqwest::Url;

use crate::tools::traits::ToolError;

pub(crate) fn parse_host(url: &str) -> Result<String, ToolError> {
    let parsed = Url::parse(url).map_err(|err| ToolError::new(err.to_string()))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err(ToolError::new("unsupported URL scheme".to_string())),
    }
    parsed
        .host_str()
        .map(|host| host.to_string())
        .ok_or_else(|| ToolError::new("missing host".to_string()))
}

pub(crate) async fn ensure_allowed_url(url: &str, host: &str) -> Result<(), ToolError> {
    let parsed = Url::parse(url).map_err(|err| ToolError::new(err.to_string()))?;
    if parsed.username() != "" || parsed.password().is_some() {
        return Err(ToolError::new(
            "credentials in URL are not allowed".to_string(),
        ));
    }
    let port = parsed.port_or_known_default().unwrap_or(80);
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|err| ToolError::new(err.to_string()))?;
    for addr in addrs {
        if is_private_ip(addr.ip()) {
            return Err(ToolError::new(format!(
                "SSRF blocked: {host} resolves to private IP {}",
                addr.ip()
            )));
        }
    }
    Ok(())
}

pub(crate) fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.octets()[0] == 169
        }
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}
