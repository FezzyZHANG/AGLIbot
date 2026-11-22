use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{Result, anyhow, ensure};
use reqwest::{Response, StatusCode, Url, header::LOCATION};

use super::{Attachment, AttachmentKind};

const MAX_REDIRECTS: usize = 3;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(20);

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
    tokio::time::timeout(DOWNLOAD_TIMEOUT, download_with(input, max, request_public))
        .await.map_err(|_| anyhow!("平台附件下载超时"))?
}

async fn request_public(url: Url, addresses: Vec<SocketAddr>) -> Result<Response> {
    let http = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .resolve_to_addrs(url.host_str().unwrap(), &addresses)
        .redirect(reqwest::redirect::Policy::none())
        .build().map_err(reqwest::Error::without_url)?;
    Ok(http.get(url).send().await.map_err(reqwest::Error::without_url)?)
}

async fn download_with<F, Fut>(input: &str, max: usize, mut request: F) -> Result<Vec<u8>>
where
    F: FnMut(Url, Vec<SocketAddr>) -> Fut,
    Fut: Future<Output = Result<Response>>,
{
    let (mut url, mut addresses) = public_url(input).await?;
    let mut hops = 0;
    loop {
        let response = request(url.clone(), addresses).await?;
        match response.status() {
            StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT => {
                ensure!(hops < MAX_REDIRECTS, "平台附件重定向次数过多");
                let location = response.headers().get(LOCATION).and_then(|value| value.to_str().ok())
                    .map(str::trim).filter(|value| !value.is_empty() && value.len() <= 4096)
                    .ok_or_else(|| anyhow!("平台附件重定向缺少有效地址"))?;
                let target = url.join(location).map_err(|_| anyhow!("平台附件重定向地址无效"))?;
                (url, addresses) = public_url(target.as_str()).await?;
                hops += 1;
            }
            status if status.is_success() => return super::read_capped(response, max).await,
            _ => return Err(anyhow!("平台附件下载失败")),
        }
    }
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

    fn response(status: StatusCode, location: Option<&str>, body: Vec<u8>) -> Response {
        let mut response = axum::http::Response::builder().status(status);
        if let Some(location) = location {
            response = response.header(LOCATION, location);
        }
        response.body(body).unwrap().into()
    }

    #[tokio::test]
    async fn downloads_stop_at_the_limit_and_errors_hide_the_url() {
        let url = "https://8.8.8.8/secret-token";
        for (status, size) in [(StatusCode::OK, 8), (StatusCode::OK, 9), (StatusCode::FORBIDDEN, 0)] {
            let result = download_with(url, 8, |_, _| async move { Ok(response(status, None, vec![7; size])) }).await;
            match size {
                8 => assert_eq!(result.unwrap(), vec![7; 8]),
                9 => assert!(result.unwrap_err().is::<super::super::TooLarge>()),
                _ => assert!(!format!("{:#}", result.unwrap_err()).contains("secret-token")),
            }
        }

        // A response without a known length must also be stopped while reading.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = format!("http://{}/secret-token", listener.local_addr().unwrap());
        let serve = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            while !received.ends_with(b"\r\n\r\n") {
                received.push(socket.read_u8().await.unwrap());
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n123456789").await.unwrap();
        });
        let response = reqwest::Client::builder().no_proxy().build().unwrap().get(url).send().await.unwrap();
        assert_eq!(response.content_length(), None);
        let error = super::super::read_capped(response, 8).await.unwrap_err();
        assert!(error.is::<super::super::TooLarge>());
        serve.await.unwrap();
        for target in ["file:///C:/Windows/win.ini", "C:/Windows/win.ini"] {
            assert!(download(target, 8).await.is_err());
        }
    }

    #[tokio::test]
    async fn follows_three_validated_redirects_including_relative_locations() {
        for status in [StatusCode::MOVED_PERMANENTLY, StatusCode::FOUND, StatusCode::SEE_OTHER,
            StatusCode::TEMPORARY_REDIRECT, StatusCode::PERMANENT_REDIRECT] {
            let mut hops = [
                ("https://8.8.8.8/a/start", Some("next")),
                ("https://8.8.8.8/a/next", Some("//1.1.1.1/public")),
                ("https://1.1.1.1/public", Some("/final")),
                ("https://1.1.1.1/final", None),
            ].into_iter();
            let bytes = download_with("https://8.8.8.8/a/start", 8, |url, addresses| {
                let (expected, location) = hops.next().unwrap();
                assert_eq!(url.as_str(), expected);
                assert_eq!(addresses, [SocketAddr::new(url.host_str().unwrap().parse().unwrap(), 443)]);
                async move {
                    Ok(if location.is_some() { response(status, location, vec![]) }
                        else { response(StatusCode::OK, None, vec![42]) })
                }
            }).await.unwrap();
            assert!(hops.next().is_none());
            assert_eq!(bytes, vec![42]);
        }
    }

    #[tokio::test]
    async fn rejects_excess_redirects() {
        let mut requests = 0;
        let result = download_with("https://8.8.8.8/start", 8, |_, _| {
            requests += 1;
            async { Ok(response(StatusCode::FOUND, Some("/another"), vec![])) }
        }).await;
        assert!(result.unwrap_err().to_string().contains("次数过多"));
        assert_eq!(requests, 4);
    }

    #[tokio::test]
    async fn rejects_missing_invalid_and_non_public_redirect_locations_before_requesting_them() {
        for location in [None, Some(" "), Some("http://[secret-token"), Some("http://127.0.0.1/secret-token"),
            Some("//10.0.0.1/secret-token"), Some("http://[::ffff:127.0.0.1]/secret-token"),
            Some("http://169.254.169.254/secret-token"), Some("file:///secret-token")] {
            let mut requests = 0;
            let result = download_with("https://8.8.8.8/start", 8, |_, _| {
                requests += 1;
                async { Ok(response(StatusCode::FOUND, location, vec![])) }
            }).await;
            let error = result.unwrap_err();
            assert!(!format!("{error:#}").contains("secret-token"));
            assert_eq!(requests, 1);
        }
    }

    /// A 4xx still proves the handshake with the QQ image CDN.
    #[tokio::test]
    #[ignore = "requires network"]
    async fn live_qq_cdn_handshake() {
        let (url, addresses) = public_url("https://multimedia.nt.qq.com.cn/download?appid=1407").await.unwrap();
        let response = tokio::time::timeout(DOWNLOAD_TIMEOUT, request_public(url, addresses)).await.unwrap().unwrap();
        println!("status {}", response.status());
    }

    #[tokio::test]
    async fn excludes_non_public_targets() {
        for url in ["file:///etc/passwd", "http://127.1", "http://10.0.0.1", "http://169.254.169.254", "http://[::ffff:127.0.0.1]", "http://host.local", "https://user@8.8.8.8", "http://8.8.8.8:123"] {
            assert!(public_url(url).await.is_err(), "{url}");
        }
        assert!(public_url("https://8.8.8.8").await.is_ok());
        assert_eq!(authored_links("请看 https://example.com/p。").first().unwrap().url.as_deref(), Some("https://example.com/p"));
    }
}
