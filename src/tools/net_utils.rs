use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use futures::StreamExt;
use reqwest::Url;

use crate::tools::traits::{ToolContext, ToolError};

pub fn parse_host(url: &str) -> Result<String, ToolError> {
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

pub async fn ensure_allowed_url(
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
    if port == 0 {
        return Err(ToolError::new("invalid port 0".to_string()));
    }
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|err| ToolError::new(err.to_string()))?;
    for addr in addrs {
        if is_private_ip(addr.ip()) {
            tracing::warn!(
                event = "ssrf_blocked",
                reason = "non_global_ip",
                host = %host,
                ip = %addr.ip(),
                user_id = ?user_id,
                session_id = ?session_id,
                channel_id = ?channel_id,
                "SSRF blocked: host resolves to non-global IP"
            );
            return Err(ToolError::new(format!(
                "SSRF blocked: {host} resolves to non-global IP {}",
                addr.ip()
            )));
        }
    }
    Ok(())
}

pub async fn read_response_bytes(
    response: reqwest::Response,
    max_bytes: u64,
    kind: &str,
) -> Result<Vec<u8>, ToolError> {
    if max_bytes == 0 {
        return Err(ToolError::new(format!(
            "{kind} is too large: limit is 0 bytes"
        )));
    }
    if let Some(length) = response.content_length() {
        if length > max_bytes {
            return Err(ToolError::new(format!(
                "{kind} is too large: {length} bytes (limit {max_bytes})"
            )));
        }
    }

    let capacity = response
        .content_length()
        .map(|len| std::cmp::min(len, max_bytes))
        .and_then(|len| usize::try_from(len).ok())
        .unwrap_or(8192);
    let mut buffer = Vec::with_capacity(capacity);
    let mut stream = response.bytes_stream();
    let mut size: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| ToolError::new(err.to_string()))?;
        let new_size = size.saturating_add(chunk.len() as u64);
        if new_size > max_bytes {
            return Err(ToolError::new(format!(
                "{kind} exceeded limit of {max_bytes} bytes"
            )));
        }
        buffer.extend_from_slice(&chunk);
        size = new_size;
    }
    Ok(buffer)
}

pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_non_global_ipv4(v4),
        IpAddr::V6(v6) => is_non_global_ipv6(v6),
    }
}

fn is_non_global_ipv4(v4: Ipv4Addr) -> bool {
    let octets = v4.octets();
    v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_unspecified()
        || v4.is_multicast()
        || octets[0] == 0
        || (octets[0] == 100 && (octets[1] & 0b1100_0000) == 0b0100_0000)
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
        || (octets[0] == 198 && (octets[1] & 0b1111_1110) == 18)
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
        || octets[0] >= 240
}

fn is_non_global_ipv6(v6: Ipv6Addr) -> bool {
    let segments = v6.segments();
    let seg0 = segments[0];
    let seg1 = segments[1];
    if v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        || (seg0 & 0xffc0) == 0xfe80
        || (seg0 & 0xffc0) == 0xfec0
        || (seg0 & 0xfe00) == 0xfc00
        || (seg0 == 0x2001 && seg1 == 0x0db8)
        || (seg0 == 0x2001 && seg1 == 0x0000)
    {
        return true;
    }

    if let Some(v4) = v6.to_ipv4_mapped() {
        return is_non_global_ipv4(v4);
    }

    if let Some(v4) = ipv4_compatible_v6(v6) {
        return is_non_global_ipv4(v4);
    }

    if let Some(v4) = nat64_embedded_ipv4(v6) {
        return is_non_global_ipv4(v4);
    }

    false
}

fn ipv4_compatible_v6(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = v6.segments();
    if segments[0] == 0
        && segments[1] == 0
        && segments[2] == 0
        && segments[3] == 0
        && segments[4] == 0
        && segments[5] == 0
    {
        let high = segments[6];
        let low = segments[7];
        if high == 0 && (low == 0 || low == 1) {
            return None;
        }
        return Some(Ipv4Addr::new(
            (high >> 8) as u8,
            high as u8,
            (low >> 8) as u8,
            low as u8,
        ));
    }
    None
}

fn nat64_embedded_ipv4(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = v6.segments();
    if segments[0] == 0x0064
        && segments[1] == 0xff9b
        && segments[2] == 0
        && segments[3] == 0
        && segments[4] == 0
        && segments[5] == 0
    {
        let high = segments[6];
        let low = segments[7];
        return Some(Ipv4Addr::new(
            (high >> 8) as u8,
            high as u8,
            (low >> 8) as u8,
            low as u8,
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use reqwest::Client;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    use super::{ensure_allowed_url, is_private_ip, parse_host, read_response_bytes};

    #[test]
    fn ipv4_non_global_ranges_blocked() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(240, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1))));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }

    #[test]
    fn ipv6_non_global_ranges_blocked() {
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0xfc00, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0xfec0, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0x0db8, 0, 0, 0, 0, 0, 1
        ))));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0x0000, 0, 0, 0, 0, 0, 1
        ))));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0x0064, 0xff9b, 0, 0, 0, 0, 0x0a00, 0x0001
        ))));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0, 0, 0, 0, 0, 0xffff, 0xc0a8, 0x0001
        ))));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0, 0, 0, 0, 0, 0, 0x0a00, 0x0001
        ))));
        assert!(!is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888
        ))));
    }

    #[tokio::test]
    async fn ensure_allowed_url_blocks_ipv6_literal() {
        let url = "http://[::1]/";
        let host = parse_host(url).unwrap();
        let result = ensure_allowed_url(url, &host, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_response_bytes_allows_small_body() {
        let body = vec![b'a'; 256];
        let url = spawn_server(body.clone(), true).await;
        let response = Client::new().get(url).send().await.unwrap();
        let bytes = read_response_bytes(response, 1024, "response")
            .await
            .unwrap();
        assert_eq!(bytes, body);
    }

    #[tokio::test]
    async fn read_response_bytes_rejects_over_limit_with_content_length() {
        let body = vec![b'a'; 2048];
        let url = spawn_server(body, true).await;
        let response = Client::new().get(url).send().await.unwrap();
        let result = read_response_bytes(response, 1024, "response").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_response_bytes_rejects_over_limit_without_content_length() {
        let body = vec![b'a'; 2048];
        let url = spawn_server(body, false).await;
        let response = Client::new().get(url).send().await.unwrap();
        let result = read_response_bytes(response, 1024, "response").await;
        assert!(result.is_err());
    }

    async fn spawn_server(body: Vec<u8>, include_length: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut headers = String::from(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n",
                );
                if include_length {
                    headers.push_str(&format!("Content-Length: {}\r\n", body.len()));
                }
                headers.push_str("\r\n");
                let _ = socket.write_all(headers.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            }
        });
        format!("http://{addr}")
    }
}
