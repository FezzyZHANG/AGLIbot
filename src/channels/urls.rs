use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{Result, anyhow, ensure};
use reqwest::Url;
use super::{Attachment, AttachmentKind};

// Permit globally routed unicast only. IPv6 transition addresses can tunnel IPv4.
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(matches!(a, 0 | 10 | 127 | 224..=255)
                || matches!((a, b), (100, 64..=127) | (169, 254) | (172, 16..=31) | (192, 168) | (198, 18..=19))
                || matches!((a, b, c), (192, 0, 0 | 2) | (192, 88, 99) | (198, 51, 100) | (203, 0, 113)))
        }
        IpAddr::V6(ip) => {
            let [a, b, ..] = ip.segments();
            a & 0xe000 == 0x2000 && a != 0x2002 && !(a == 0x2001 && (b <= 0x1ff || b == 0xdb8))
                && !(a == 0x3fff && b <= 0xfff)
        }
    }
}

pub(crate) async fn public_url(input: &str) -> Result<(Url, Vec<SocketAddr>)> {
    ensure!(input.len() <= 4096, "链接太长");
    let mut url = Url::parse(input).map_err(|_| anyhow!("链接格式无效"))?;
    ensure!(matches!(url.scheme(), "https" | "http") && url.username().is_empty() && url.password().is_none() && url.port().is_none(), "仅支持不含登录信息、使用默认端口的公网 HTTP(S) 链接");
    url.set_fragment(None);
    let host = url.host_str().ok_or_else(|| anyhow!("链接缺少主机"))?.trim_matches(['[', ']']);
    let port = url.port_or_known_default().unwrap();
    let addresses = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        let host = host.trim_end_matches('.');
        ensure!(host.contains('.') && ![".local", ".localhost", ".internal", ".home.arpa", ".onion"].iter().any(|s| host.ends_with(s)), "仅支持公网主机");
        tokio::time::timeout(Duration::from_secs(4), tokio::net::lookup_host((host, port))).await
            .map_err(|_| anyhow!("域名解析超时"))?.map_err(|_| anyhow!("域名解析失败"))?.take(65).collect()
    };
    ensure!(!addresses.is_empty() && addresses.len() <= 64 && addresses.iter().all(|s| public_ip(s.ip())), "仅支持公网地址");
    Ok((url, addresses))
}

pub(super) async fn download(input: &str, max: usize) -> Result<Vec<u8>> {
    let (url, addresses) = public_url(input).await?;
    let http = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(20))
        .resolve_to_addrs(url.host_str().unwrap(), &addresses)
        .redirect(reqwest::redirect::Policy::none()).build()?;
    let response = http.get(url).send().await.map_err(reqwest::Error::without_url)?;
    ensure!(response.status().is_success(), "平台附件下载失败");
    super::read_capped(response, max).await
}

pub(super) fn authored_links(text: &str) -> Vec<Attachment> {
    let mut links = Vec::new();
    let mut consumed = 0;
    for (start, _) in text.match_indices("http") {
        if start < consumed || text[..start].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') { continue; }
        let token = &text[start..];
        if !(token.starts_with("https://") || token.starts_with("http://")) { continue; }
        let end = token.find(|c: char| c.is_whitespace() || "<>\"'。，！？、【】「」《》（）".contains(c)).unwrap_or(token.len());
        consumed = start + end;
        let mut url = token[..end].trim_end_matches(['.', ',', ';', '!', '?', ']', '}']);
        if url.ends_with(')') && url.matches(')').count() > url.matches('(').count() { url = url.trim_end_matches(')'); }
        if url.len() > 4096 { continue; }
        if links.iter().any(|a: &Attachment| a.url.as_deref() == Some(url)) { continue; }
        links.push(Attachment { kind: AttachmentKind::Link, url: Some(url.into()), file_id: None, name: None, size: None, quoted_from: None, media_id: None });
        if links.len() == 4 { break; }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn excludes_non_public_targets() {
        for url in ["file:///etc/passwd", "http://127.1", "http://10.0.0.1", "http://169.254.169.254", "http://[::ffff:127.0.0.1]", "http://host.local", "https://user@8.8.8.8", "http://8.8.8.8:123"] {
            assert!(public_url(url).await.is_err(), "{url}");
        }
        assert!(public_url("https://8.8.8.8").await.is_ok());
        assert_eq!(authored_links("请看 https://example.com/p。").first().unwrap().url.as_deref(), Some("https://example.com/p"));
    }
}
