//! The proxies `HTTP_PROXY`, `HTTPS_PROXY` and `NO_PROXY` name, read as Scramjet 1's undici
//! (`EnvHttpProxyAgent`) reads them: each lower-case name before its upper-case one,
//! `HTTPS_PROXY` falling back to `HTTP_PROXY`, and every request through a proxy in a
//! `CONNECT` tunnel. Unlike undici, servers on this machine are never reached through one: a
//! proxy elsewhere cannot reach them, and a plain `http://` request would show it their keys.

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const HEAD_BYTES: usize = 16 << 10;

#[derive(Debug, PartialEq)]
pub(super) struct Proxy {
    host: String,
    port: u16,
    /// Its host and port without credentials, for messages.
    shown: String,
    authorization: Option<String>,
}

/// The proxy that requests to `host:port` go through, if the environment names one.
pub(super) fn for_host(tls: bool, host: &str, port: u16) -> Result<Option<Proxy>, String> {
    chosen(|name| std::env::var(name).ok(), tls, host, port)
}

fn chosen(
    variable: impl Fn(&str) -> Option<String>,
    tls: bool,
    host: &str,
    port: u16,
) -> Result<Option<Proxy>, String> {
    // As undici's `??`: a lower-case name set, even empty, hides the upper-case one.
    let read = |lower: &str, upper: &'static str| {
        let value = variable(lower).or_else(|| variable(upper))?;
        (!value.is_empty()).then_some((upper, value))
    };
    let http = read("http_proxy", "HTTP_PROXY");
    let named = match tls {
        true => read("https_proxy", "HTTPS_PROXY").or(http),
        false => http,
    };
    let Some((name, url)) = named else {
        return Ok(None);
    };
    let host = host.to_ascii_lowercase();
    let no_proxy = variable("no_proxy").or_else(|| variable("NO_PROXY"));
    if local(&host) || exempt(&no_proxy.unwrap_or_default(), &host, port) {
        return Ok(None);
    }
    parse(name, &url).map(Some)
}

fn local(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || (host.parse::<std::net::IpAddr>()).is_ok_and(|ip| ip.is_loopback())
}

/// Whether `NO_PROXY`'s entries (`host`, `.domain`, `*.domain`, each perhaps with `:port`,
/// or `*`) take `host:port` out of the proxy's way: the host itself and its subdomains.
fn exempt(no_proxy: &str, host: &str, port: u16) -> bool {
    let entries = no_proxy.split(|c: char| c == ',' || c.is_whitespace());
    entries.filter(|entry| !entry.is_empty()).any(|entry| {
        if entry == "*" {
            return true;
        }
        let (name, wanted) = match entry.rsplit_once(':') {
            Some((name, digits))
                if !name.is_empty()
                    && !digits.is_empty()
                    && digits.bytes().all(|b| b.is_ascii_digit()) =>
            {
                (name, digits.parse::<u16>().ok())
            }
            _ => (entry, None),
        };
        let name = name.strip_prefix('*').unwrap_or(name);
        let name = name.strip_prefix('.').unwrap_or(name);
        let name = name.trim_start_matches('[').trim_end_matches(']');
        let name = name.to_ascii_lowercase();
        wanted.is_none_or(|wanted| wanted == port)
            && (host == name
                || host
                    .strip_suffix(name.as_str())
                    .is_some_and(|sub| sub.ends_with('.')))
    })
}

/// `url` (`http://user:password@host:port`, or without the scheme) as `name` gives it. Its
/// messages never repeat it, as it may hold a password.
fn parse(name: &str, url: &str) -> Result<Proxy, String> {
    let rest = match url.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
        Some((scheme, _)) => {
            return Err(format!(
                "{name}: a {scheme}:// proxy is not supported, only http://"
            ));
        }
        None => url,
    };
    let rest = &rest[..rest.find('/').unwrap_or(rest.len())];
    let (userinfo, authority) = match rest.rsplit_once('@') {
        Some((userinfo, authority)) => (Some(userinfo), authority),
        None => (None, rest),
    };
    let (host, port) = match authority.strip_prefix('[') {
        Some(bracketed) => match bracketed.split_once(']') {
            Some((host, after)) => (host, after.strip_prefix(':')),
            None => (bracketed, None),
        },
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    if host.is_empty() {
        return Err(format!("{name}: the proxy's URL has no host"));
    }
    let port = match port {
        None | Some("") => 80,
        Some(port) => {
            (port.parse()).map_err(|_| format!("{name}: the proxy's port is not a number"))?
        }
    };
    let authorization = userinfo.and_then(|userinfo| {
        let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
        let credentials = [decoded(user), decoded(password)].join(":");
        (!user.is_empty())
            .then(|| format!("Basic {}", super::image::base64(credentials.as_bytes())))
    });
    Ok(Proxy {
        host: host.to_owned(),
        port,
        shown: authority.to_owned(),
        authorization,
    })
}

/// `text` with its `%XX` escapes decoded, as a URL's user and password are written.
fn decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = (bytes.get(at + 1..at + 3))
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .filter(|_| bytes[at] == b'%')
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match hex {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A connection to `host:port` through `proxy`'s tunnel, `whom` naming it in failures, which
/// say whether another attempt might succeed.
pub(super) async fn tunnel(
    proxy: &Proxy,
    host: &str,
    port: u16,
    whom: &str,
) -> Result<TcpStream, (String, bool)> {
    let shown = &proxy.shown;
    let broke = |e: std::io::Error| (format!("the proxy {shown} for {whom} failed: {e}"), true);
    let mut socket =
        (TcpStream::connect((proxy.host.as_str(), proxy.port)).await).map_err(|e| {
            (
                format!("cannot connect to the proxy {shown} for {whom}: {e}"),
                true,
            )
        })?;
    let target = match host.contains(':') {
        true => format!("[{host}]:{port}"),
        false => format!("{host}:{port}"),
    };
    let mut request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some(authorization) = &proxy.authorization {
        request.push_str(&format!("Proxy-Authorization: {authorization}\r\n"));
    }
    request.push_str("\r\n");
    socket.write_all(request.as_bytes()).await.map_err(broke)?;
    // A byte at a time: what follows the head is the tunnel's, read by TLS or HTTP.
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > HEAD_BYTES {
            return Err((
                format!("the proxy {shown} for {whom} answered with a head over 16 KB"),
                false,
            ));
        }
        let mut byte = [0];
        match socket.read(&mut byte).await.map_err(broke)? {
            0 => {
                return Err((
                    format!("the proxy {shown} for {whom} closed the connection"),
                    true,
                ));
            }
            _ => head.push(byte[0]),
        }
    }
    let head = String::from_utf8_lossy(&head);
    let status = head.lines().next().unwrap_or_default().trim();
    let code = (status.split_whitespace().nth(1)).and_then(|code| code.parse::<u16>().ok());
    match code {
        Some(200..=299) => Ok(socket),
        code => {
            let hint = match (code, &proxy.authorization) {
                (Some(407), None) => {
                    "; give its user and password in its URL: http://user:password@host:port"
                }
                _ => "",
            };
            Err((
                format!("the proxy {shown} refused to connect to {whom}: {status}{hint}"),
                matches!(code, Some(502..=504)),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn with(
        pairs: &[(&str, &str)],
        tls: bool,
        host: &str,
        port: u16,
    ) -> Result<Option<Proxy>, String> {
        let env: HashMap<String, String> = (pairs.iter())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        chosen(|name| env.get(name).cloned(), tls, host, port)
    }

    fn shown(proxy: Result<Option<Proxy>, String>) -> Option<String> {
        proxy
            .unwrap()
            .map(|proxy| format!("{}:{}", proxy.host, proxy.port))
    }

    #[test]
    fn the_variables_are_read_as_scramjet_1_reads_them() {
        let https = |pairs: &[(&str, &str)]| shown(with(pairs, true, "api.example.com", 443));
        let http = |pairs: &[(&str, &str)]| shown(with(pairs, false, "api.example.com", 80));
        assert_eq!(https(&[]), None);
        assert_eq!(
            https(&[("HTTPS_PROXY", "http://p:3128")]).unwrap(),
            "p:3128"
        );
        assert_eq!(https(&[("HTTP_PROXY", "p")]).unwrap(), "p:80");
        assert_eq!(
            https(&[("HTTPS_PROXY", "s:1"), ("HTTP_PROXY", "h:2")]).unwrap(),
            "s:1"
        );
        assert_eq!(http(&[("HTTPS_PROXY", "s:1")]), None);
        assert_eq!(
            http(&[("http_proxy", "l:1"), ("HTTP_PROXY", "u:2")]).unwrap(),
            "l:1"
        );
        // Set but empty, the lower-case name still hides the upper-case one.
        assert_eq!(http(&[("http_proxy", ""), ("HTTP_PROXY", "u:2")]), None);
        assert_eq!(https(&[("HTTPS_PROXY", "[::1]:8080")]).unwrap(), "::1:8080");
    }

    #[test]
    fn no_proxy_and_this_machine_go_direct() {
        let proxied = |no_proxy: &str, host: &str, port: u16| {
            let pairs = [("HTTPS_PROXY", "p:1"), ("NO_PROXY", no_proxy)];
            shown(with(&pairs, true, host, port)).is_some()
        };
        assert!(proxied("", "api.example.com", 443));
        assert!(!proxied("*", "api.example.com", 443));
        assert!(!proxied("example.com", "api.example.com", 443));
        assert!(!proxied(".example.com", "EXAMPLE.com", 443));
        assert!(!proxied("other, *.example.com", "a.b.example.com", 443));
        assert!(proxied("example.com", "badexample.com", 443));
        assert!(!proxied("example.com:443", "example.com", 443));
        assert!(proxied("example.com:8443", "example.com", 443));
        assert!(!proxied("10.0.0.1", "10.0.0.1", 443));
        for host in ["localhost", "127.0.0.1", "127.8.0.1", "::1", "a.localhost"] {
            assert!(!proxied("", host, 18080), "{host}");
        }
        let lower = [
            ("HTTPS_PROXY", "p:1"),
            ("no_proxy", "a.com"),
            ("NO_PROXY", "b.com"),
        ];
        assert!(shown(with(&lower, true, "b.com", 443)).is_some());
    }

    #[test]
    fn a_proxy_url_gives_its_credentials_and_its_failures_hide_them() {
        let proxy = parse("HTTPS_PROXY", "http://me%40corp:p%3Ass@proxy:3128/").unwrap();
        assert_eq!((proxy.host.as_str(), proxy.port), ("proxy", 3128));
        assert_eq!(proxy.shown, "proxy:3128");
        // base64 of "me@corp:p:ss".
        assert_eq!(proxy.authorization.unwrap(), "Basic bWVAY29ycDpwOnNz");
        assert_eq!(parse("X", "HTTP://p").unwrap().authorization, None);
        for (url, problem) in [
            (
                "https://u:secret@p:1",
                "HTTPS_PROXY: a https:// proxy is not supported, only http://",
            ),
            (
                "socks5://p:1",
                "HTTPS_PROXY: a socks5:// proxy is not supported, only http://",
            ),
            (
                "http://u:secret@p:port",
                "HTTPS_PROXY: the proxy's port is not a number",
            ),
            (
                "http://u:secret@",
                "HTTPS_PROXY: the proxy's URL has no host",
            ),
        ] {
            assert_eq!(parse("HTTPS_PROXY", url).unwrap_err(), problem);
        }
    }

    /// A proxy answering one CONNECT with `answer`, then, after a 2xx, echoing a line back;
    /// gives its address and the request it read.
    fn proxy_answering(answer: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let (sent, read) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            while !request.ends_with("\r\n\r\n") {
                reader.read_line(&mut request).unwrap();
            }
            sent.send(request).unwrap();
            stream.write_all(answer.as_bytes()).unwrap();
            let mut line = String::new();
            if reader.read_line(&mut line).is_ok() {
                let _ = stream.write_all(format!("echo {line}").as_bytes());
            }
        });
        (address, read)
    }

    fn tunnelled(proxy: &str, answer: &'static str) -> (Result<String, (String, bool)>, String) {
        let (address, request) = proxy_answering(answer);
        let proxy = parse("HTTPS_PROXY", &proxy.replace("ADDRESS", &address)).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(async {
            let mut socket = tunnel(
                &proxy,
                "api.example.com",
                443,
                "api.example.com (provider p)",
            )
            .await?;
            socket.write_all(b"hello\n").await.unwrap();
            let mut echoed = String::new();
            socket.read_to_string(&mut echoed).await.unwrap();
            Ok::<_, (String, bool)>(echoed)
        });
        let result =
            result.map_err(|(problem, again)| (problem.replace(&address, "ADDRESS"), again));
        (result, request.recv().unwrap())
    }

    #[test]
    fn a_tunnel_carries_the_connection_after_the_proxy_agrees() {
        let (echoed, request) = tunnelled(
            "http://u:p@ADDRESS",
            "HTTP/1.1 200 Connection established\r\n\r\n",
        );
        assert_eq!(echoed.unwrap(), "echo hello\n");
        assert_eq!(
            request,
            "CONNECT api.example.com:443 HTTP/1.1\r\nHost: api.example.com:443\r\n\
             Proxy-Authorization: Basic dTpw\r\n\r\n"
        );
        let (refused, _) = tunnelled(
            "ADDRESS",
            "HTTP/1.1 407 Proxy Authentication Required\r\n\r\n",
        );
        assert_eq!(
            refused.unwrap_err(),
            (
                "the proxy ADDRESS refused to connect to api.example.com (provider p): HTTP/1.1 \
                 407 Proxy Authentication Required; give its user and password in its URL: \
                 http://user:password@host:port"
                    .to_owned(),
                false
            )
        );
        let (unreachable, _) = tunnelled("ADDRESS", "HTTP/1.1 503 Unavailable\r\n\r\n");
        assert!(unreachable.unwrap_err().1);
    }
}
