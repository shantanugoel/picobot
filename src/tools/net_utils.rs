use std::net::IpAddr;

use reqwest::Url;

use crate::tools::traits::{ToolContext, ToolError};

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

pub(crate) async fn ensure_allowed_url(
    url: &str,
    host: &str,
    ctx: Option<&ToolContext>,
) -> Result<(), ToolError> {
    let parsed = Url::parse(url).map_err(|err| ToolError::new(err.to_string()))?;
    let (user_id, session_id, channel_id) = ctx
        .map(|ctx| {
            (
                ctx.user_id.as_deref(),
                ctx.session_id.as_deref(),
                ctx.channel_id.as_deref(),
            )
        })
        .unwrap_or((None, None, None));
    if parsed.username() != "" || parsed.password().is_some() {
        tracing::warn!(
            event = "ssrf_blocked",
            reason = "credentials",
            host = %host,
            user_id = ?user_id,
            session_id = ?session_id,
            channel_id = ?channel_id,
            "credentials in URL are not allowed"
        );
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
            tracing::warn!(
                event = "ssrf_blocked",
                reason = "private_ip",
                host = %host,
                ip = %addr.ip(),
                user_id = ?user_id,
                session_id = ?session_id,
                channel_id = ?channel_id,
                "SSRF blocked: host resolves to private IP"
            );
            return Err(ToolError::new(format!(
                "SSRF blocked: {host} resolves to private IP {}",
                addr.ip()
            )));
        }
    }
    Ok(())
}

pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.octets()[0] == 169
        }
        IpAddr::V6(v6) => {
            let seg0 = v6.segments()[0];
            let seg1 = v6.segments()[1];
            v6.is_loopback()
                || v6.is_unspecified()
                || (seg0 & 0xffc0) == 0xfe80
                || (seg0 & 0xfe00) == 0xfc00
                || (seg0 == 0x2001 && seg1 == 0x0db8)
                || v6
                    .to_ipv4_mapped()
                    .map(|v4| is_private_ip(IpAddr::V4(v4)))
                    .unwrap_or(false)
        }
    }
}
