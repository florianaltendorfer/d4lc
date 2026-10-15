//! Bounded HTTP GET with a URL policy checked on every redirect hop.
//!
//! Every `ureq` request of the app (preset catalog, preset archives,
//! Maxroll and InfinityBuilds builds, the release check) goes through
//! [`fetch_bounded_with_timeout`]: the response body is capped at a
//! caller-chosen size while it streams, error bodies are capped at
//! [`MAX_ERROR_BODY_BYTES`], and redirects are followed manually so the host
//! policy also holds for every `Location` target (ureq's own redirect
//! handling would follow any host, port or scheme). The browser-driven
//! provider imports do not use this path.

use super::preset_download::DownloadError;

/// Redirect hops followed after the initial request. The real flows need one
/// (`github.com/.../raw/...` → `raw.githubusercontent.com`).
pub(crate) const MAX_REDIRECTS: usize = 3;

/// Cap on the bytes kept from a non-success response body. The body only
/// feeds diagnostics and the catalog's "blank or not" classification.
pub(crate) const MAX_ERROR_BODY_BYTES: u64 = 64 * 1024;

/// Overall deadline of one preset or provider fetch: connect, TLS, every
/// redirect hop and the body read. DNS resolution is bounded only by the
/// system resolver (ureq 2 applies no deadline to it). Upstream performs the
/// same fetches through .NET `HttpClient`, whose default request timeout is
/// 100 s; ureq 2.x has no default timeout.
pub(crate) const DOWNLOAD_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(100);

/// Hosts a download may contact.
#[derive(Debug, Clone, Copy)]
enum HostRule {
    /// Exactly one of these lowercase host names.
    Exact(&'static [&'static str]),
    /// This lowercase domain or any subdomain of it.
    DomainSuffix(&'static str),
}

/// URL policy applied to the initial URL and to every redirect target:
/// `https` only, no userinfo, the scheme's default port, a DNS host name
/// (no IP literal) matching the host rule. Authorities registered with
/// [`UrlPolicy::with_local_authority`] are allowed verbatim over `http` or
/// `https` (local stub servers behind the `D4LC_PRESET_DOWNLOAD_BASE_URL`
/// override and in tests).
#[derive(Debug, Clone)]
pub(crate) struct UrlPolicy {
    hosts: HostRule,
    local_authorities: Vec<String>,
}

/// The parts of an accepted URL the fetch loop needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckedUrl {
    pub https: bool,
    /// Lowercase host as written in the URL (IPv6 literals keep brackets).
    pub host: String,
    /// Effective port (explicit or the scheme default).
    pub port: u16,
}

impl UrlPolicy {
    /// Allow exactly the given lowercase host names.
    pub(crate) fn exact_hosts(hosts: &'static [&'static str]) -> Self {
        Self {
            hosts: HostRule::Exact(hosts),
            local_authorities: Vec::new(),
        }
    }

    /// Allow `domain` and its subdomains.
    pub(crate) fn domain_suffix(domain: &'static str) -> Self {
        Self {
            hosts: HostRule::DomainSuffix(domain),
            local_authorities: Vec::new(),
        }
    }

    /// Additionally allow exactly `authority` (`host:port` with an explicit
    /// port, as returned by [`effective_authority`]), over `http` too.
    pub(crate) fn with_local_authority(mut self, authority: &str) -> Self {
        self.local_authorities.push(authority.to_ascii_lowercase());
        self
    }

    /// Check `url` against the policy.
    pub(crate) fn check(&self, url: &str) -> Result<CheckedUrl, DownloadError> {
        let parsed = parse_url(url)?;
        let host = &parsed.checked.host;
        let authority = format!("{host}:{}", parsed.checked.port);
        if self.local_authorities.contains(&authority) {
            return Ok(parsed.checked);
        }
        let allowed = !is_ip_literal(host)
            && match self.hosts {
                HostRule::Exact(hosts) => hosts.contains(&host.as_str()),
                HostRule::DomainSuffix(domain) => {
                    host == domain
                        || host
                            .strip_suffix(domain)
                            .is_some_and(|prefix| prefix.ends_with('.'))
                }
            };
        if !allowed {
            return Err(DownloadError::UnexpectedHost(host.clone()));
        }
        if !parsed.checked.https {
            return Err(DownloadError::InvalidUrl(format!(
                "'{url}' is not https (plain http is refused)"
            )));
        }
        if parsed.explicit_port.is_some_and(|port| port != 443) {
            return Err(DownloadError::UnexpectedHost(authority));
        }
        Ok(parsed.checked)
    }

    /// Check a redirect from `from` to `to`: `to` must pass the policy and
    /// must not downgrade `https` to `http`.
    pub(crate) fn check_redirect(
        &self,
        from: &CheckedUrl,
        to: &str,
    ) -> Result<CheckedUrl, DownloadError> {
        let next = self
            .check(to)
            .map_err(|error| DownloadError::RedirectRefused(format!("'{to}': {error}")))?;
        if from.https && !next.https {
            return Err(DownloadError::RedirectRefused(format!(
                "'{to}': https to http downgrade"
            )));
        }
        Ok(next)
    }
}

/// `host:port` (port explicit even when it is the scheme default) of an
/// absolute http(s) URL the strict parser accepts; `None` otherwise.
pub(crate) fn effective_authority(url: &str) -> Option<String> {
    let parsed = parse_url(url).ok()?;
    Some(format!("{}:{}", parsed.checked.host, parsed.checked.port))
}

struct ParsedUrl {
    checked: CheckedUrl,
    explicit_port: Option<u16>,
}

/// Strict parser for absolute `http(s)://host[:port][/path][?query][#frag]`
/// URLs. Anything a WHATWG parser (the one ureq connects with) could read
/// differently is refused: control characters, whitespace, backslashes,
/// userinfo, percent-encoded or non-ASCII hosts.
fn parse_url(url: &str) -> Result<ParsedUrl, DownloadError> {
    let invalid = |reason: &str| DownloadError::InvalidUrl(format!("{reason} in '{url}'"));
    if url
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return Err(invalid("control, whitespace or backslash character"));
    }
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| invalid("missing scheme"))?;
    let https = match scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => false,
        _ => return Err(invalid(&format!("scheme '{scheme}' is not http(s)"))),
    };
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    if authority.contains('@') {
        return Err(invalid("userinfo (credentials)"));
    }
    let (host, port_text) = if authority.starts_with('[') {
        let end = authority
            .find(']')
            .ok_or_else(|| invalid("unterminated IPv6 literal"))?;
        let (host, tail) = authority.split_at(end + 1);
        match tail {
            "" => (host, None),
            _ => (
                host,
                Some(
                    tail.strip_prefix(':')
                        .ok_or_else(|| invalid("malformed authority"))?,
                ),
            ),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let host = host.to_ascii_lowercase();
    let host_chars_ok = if host.starts_with('[') {
        host.len() > 2
            && host[1..host.len() - 1]
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
    } else {
        !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    };
    if !host_chars_ok {
        return Err(invalid("malformed host"));
    }
    let explicit_port = match port_text {
        None => None,
        Some(text) if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) => Some(
            text.parse::<u16>()
                .map_err(|_| invalid("port out of range"))?,
        ),
        Some(_) => return Err(invalid("malformed port")),
    };
    let port = explicit_port.unwrap_or(if https { 443 } else { 80 });
    Ok(ParsedUrl {
        checked: CheckedUrl { https, host, port },
        explicit_port,
    })
}

/// Whether a WHATWG parser would treat `host` as an IP address: bracketed
/// IPv6, or a last label that is numeric (`127.0.0.1`, `2130706433`,
/// `0x7f.1`).
fn is_ip_literal(host: &str) -> bool {
    if host.starts_with('[') {
        return true;
    }
    let last = host
        .trim_end_matches('.')
        .rsplit('.')
        .next()
        .unwrap_or_default();
    !last.is_empty()
        && (last.chars().all(|c| c.is_ascii_digit())
            || last.starts_with("0x")
            || last.starts_with("0X"))
}

/// Request headers of the preset and provider downloads: upstream's shared
/// `HttpClientHandler` (`HttpClientHandler.cs:73-77`).
pub(crate) const UPSTREAM_HEADERS: &[(&str, &str)] =
    &[("User-Agent", "D4Companion"), ("Cache-Control", "no-cache")];

/// GET `url` under `policy` with the upstream headers and the
/// [`DOWNLOAD_HTTP_TIMEOUT`] deadline; see [`fetch_bounded_with_timeout`].
#[cfg(feature = "preset-download")]
pub(crate) fn fetch_bounded(
    policy: &UrlPolicy,
    url: &str,
    max_bytes: u64,
) -> Result<FetchedBody, DownloadError> {
    fetch_bounded_with_timeout(
        policy,
        url,
        max_bytes,
        DOWNLOAD_HTTP_TIMEOUT,
        UPSTREAM_HEADERS,
    )
}

/// A successful bounded fetch.
#[cfg(any(feature = "preset-download", feature = "update-check"))]
#[derive(Debug)]
pub(crate) struct FetchedBody {
    pub bytes: Vec<u8>,
    /// The URL that served the body, after redirects.
    pub final_url: String,
}

/// GET `url` under `policy` and return at most `max_bytes` of body.
///
/// Fails with [`DownloadError::BodyTooLarge`] as soon as a declared
/// `Content-Length` or the bytes actually read exceed `max_bytes`; a body
/// shorter than its `Content-Length` or a malformed chunk header is a
/// [`DownloadError::Transport`] error. Two truncations look like a complete
/// body at this layer: a close-delimited body (no `Content-Length`, not
/// chunked) cut short, and — a ureq 2.12 decoder limitation — a chunked body
/// whose connection closes inside chunk data (ureq 2 returns end-of-body
/// there; ureq 3 would be a major-version upgrade). The returned bytes are
/// then a strict prefix of the body, never more. Callers therefore prove
/// completeness from the content before using it: the preset archive must
/// parse as a zip (its end-of-central-directory record is the last record of
/// the file, so every strict prefix loses it), JSON must parse completely,
/// the InfinityBuilds page must contain its complete payload script.
///
/// `timeout` bounds connect, TLS, every hop and the body read; DNS
/// resolution is bounded only by the system resolver.
#[cfg(any(feature = "preset-download", feature = "update-check"))]
pub(crate) fn fetch_bounded_with_timeout(
    policy: &UrlPolicy,
    url: &str,
    max_bytes: u64,
    timeout: std::time::Duration,
    headers: &[(&str, &str)],
) -> Result<FetchedBody, DownloadError> {
    let deadline = std::time::Instant::now() + timeout;
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let mut current = url.to_string();
    let mut checked = policy.check(&current)?;
    let mut visited = vec![current.clone()];
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(DownloadError::Transport(format!(
                "timed out after {timeout:?}"
            )));
        }
        let request = headers.iter().fold(
            agent.get(&current).timeout(remaining),
            |request, (name, value)| request.set(name, value),
        );
        // The connection goes where ureq's URL parser says; it must agree
        // with the host and port the policy approved.
        let request_url = request
            .request_url()
            .map_err(|error| DownloadError::InvalidUrl(format!("'{current}': {error}")))?;
        let default_port = if checked.https { 443 } else { 80 };
        let connect_host = request_url.host().to_ascii_lowercase();
        if connect_host.trim_matches(['[', ']']) != checked.host.trim_matches(['[', ']'])
            || request_url.port().unwrap_or(default_port) != checked.port
        {
            return Err(DownloadError::InvalidUrl(format!(
                "'{current}' parses ambiguously"
            )));
        }
        let response = match request.call() {
            Ok(response) => response,
            // ureq reports 4xx/5xx as an error that still carries the response.
            Err(ureq::Error::Status(_, response)) => response,
            Err(error) => return Err(DownloadError::Transport(error.to_string())),
        };
        let status = response.status();
        if (300..400).contains(&status) {
            if visited.len() > MAX_REDIRECTS {
                return Err(DownloadError::RedirectRefused(format!(
                    "more than {MAX_REDIRECTS} redirects starting at '{url}'"
                )));
            }
            let location = response.header("Location").ok_or_else(|| {
                DownloadError::RedirectRefused(format!(
                    "HTTP {status} from '{current}' without a Location header"
                ))
            })?;
            let next = request_url
                .as_url()
                .join(location)
                .map_err(|error| {
                    DownloadError::RedirectRefused(format!("Location '{location}': {error}"))
                })?
                .to_string();
            checked = policy.check_redirect(&checked, &next)?;
            if visited.contains(&next) {
                return Err(DownloadError::RedirectRefused(format!(
                    "redirect loop at '{next}'"
                )));
            }
            visited.push(next.clone());
            current = next;
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(error_status(response));
        }
        let bytes = read_capped_body(response, max_bytes)?;
        return Ok(FetchedBody {
            bytes,
            final_url: current,
        });
    }
}

/// A non-success response with its body capped at [`MAX_ERROR_BODY_BYTES`].
/// Upstream reads the body before checking the status; a failed read leaves
/// it empty there.
#[cfg(any(feature = "preset-download", feature = "update-check"))]
fn error_status(response: ureq::Response) -> DownloadError {
    use std::io::Read as _;
    let status = response.status();
    let reason = response.status_text().to_string();
    let mut body = Vec::new();
    if response
        .into_reader()
        .take(MAX_ERROR_BODY_BYTES)
        .read_to_end(&mut body)
        .is_err()
    {
        body.clear();
    }
    DownloadError::HttpStatus {
        status,
        reason,
        body,
    }
}

#[cfg(any(feature = "preset-download", feature = "update-check"))]
fn read_capped_body(response: ureq::Response, max_bytes: u64) -> Result<Vec<u8>, DownloadError> {
    use std::io::Read as _;
    // With a content coding the declared length is the encoded size; only
    // the decoded byte count below is authoritative then.
    let identity = response
        .header("Content-Encoding")
        .is_none_or(|coding| coding.trim().eq_ignore_ascii_case("identity"));
    let declared = response
        .header("Content-Length")
        .and_then(|value| value.trim().parse::<u64>().ok());
    if let Some(declared) = declared.filter(|declared| identity && *declared > max_bytes) {
        return Err(DownloadError::BodyTooLarge {
            limit: max_bytes,
            declared: Some(declared),
        });
    }
    let mut body = Vec::new();
    response
        .into_reader()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|error| DownloadError::Transport(format!("read body: {error}")))?;
    if body.len() as u64 > max_bytes {
        return Err(DownloadError::BodyTooLarge {
            limit: max_bytes,
            declared: None,
        });
    }
    Ok(body)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests {
    use super::*;

    const GITHUB: &[&str] = &["github.com", "raw.githubusercontent.com"];

    #[test]
    fn policy_accepts_https_on_allowed_hosts() {
        let policy = UrlPolicy::exact_hosts(GITHUB);
        for url in [
            "https://github.com/josdemmers/x/raw/master/a.zip",
            "https://RAW.githubusercontent.com/a.zip",
            "https://github.com:443/a.zip",
            "https://github.com",
            "https://github.com?x#y",
        ] {
            policy
                .check(url)
                .unwrap_or_else(|error| panic!("{url}: {error}"));
        }
        let suffix = UrlPolicy::domain_suffix("maxroll.gg");
        suffix.check("https://planners.maxroll.gg/p").unwrap();
        suffix.check("https://maxroll.gg/p").unwrap();
    }

    #[test]
    fn policy_refuses_scheme_userinfo_port_ip_and_foreign_hosts() {
        let policy = UrlPolicy::exact_hosts(GITHUB);
        let invalid = [
            "http://github.com/a.zip",
            "ftp://github.com/a.zip",
            "file:///etc/passwd",
            "not-a-url",
            "https://user@github.com/a.zip",
            "https://github.com@evil.example/a.zip",
            "https://user:pw@github.com/a.zip",
            "https://github.com\\@evil.example/",
            "https://github.com/a b",
            "https://git%68ub.com/",
            "https://github.com:x/",
            "https://github.com:99999/",
            "https://[::1/",
        ];
        for url in invalid {
            assert!(
                matches!(policy.check(url), Err(DownloadError::InvalidUrl(_))),
                "{url}: {:?}",
                policy.check(url)
            );
        }
        let foreign = [
            "https://github.com:8443/a.zip",
            "https://example.com/a.zip",
            "https://github.com./a.zip",
            "https://evilgithub.com/a.zip",
            "https://github.com.evil.example/a.zip",
            "https://127.0.0.1/a.zip",
            "https://2130706433/a.zip",
            "https://0x7f.1/a.zip",
            "https://[::1]/a.zip",
            "https://localhost/a.zip",
        ];
        for url in foreign {
            assert!(
                matches!(policy.check(url), Err(DownloadError::UnexpectedHost(_))),
                "{url}: {:?}",
                policy.check(url)
            );
        }
        let suffix = UrlPolicy::domain_suffix("maxroll.gg");
        for url in [
            "https://evilmaxroll.gg/",
            "https://maxroll.gg.evil.example/",
        ] {
            assert!(
                matches!(suffix.check(url), Err(DownloadError::UnexpectedHost(_))),
                "{url}"
            );
        }
    }

    #[test]
    fn local_authority_is_exact_and_allows_plain_http() {
        let policy = UrlPolicy::exact_hosts(GITHUB).with_local_authority("127.0.0.1:4242");
        policy.check("http://127.0.0.1:4242/a.zip").unwrap();
        policy.check("https://127.0.0.1:4242/a.zip").unwrap();
        for url in [
            "http://127.0.0.1:4243/a.zip",
            "http://localhost:4242/a.zip",
            "http://127.0.0.1/a.zip",
        ] {
            assert!(policy.check(url).is_err(), "{url}");
        }
    }

    #[test]
    fn redirect_check_refuses_https_to_http_downgrade_and_foreign_targets() {
        let policy = UrlPolicy::exact_hosts(GITHUB).with_local_authority("127.0.0.1:4242");
        let from = policy.check("https://github.com/a.zip").unwrap();
        let error = policy
            .check_redirect(&from, "http://127.0.0.1:4242/a.zip")
            .unwrap_err();
        assert!(
            matches!(&error, DownloadError::RedirectRefused(m) if m.contains("downgrade")),
            "{error}"
        );
        assert!(matches!(
            policy.check_redirect(&from, "http://github.com/a.zip"),
            Err(DownloadError::RedirectRefused(_))
        ));
        assert!(matches!(
            policy.check_redirect(&from, "https://example.com/a.zip"),
            Err(DownloadError::RedirectRefused(_))
        ));
        policy
            .check_redirect(&from, "https://raw.githubusercontent.com/a.zip")
            .expect("the GitHub raw redirect is allowed");
    }

    /// Loopback HTTP stub servers, shared with the release-check tests.
    #[cfg(any(feature = "preset-download", feature = "update-check"))]
    pub(crate) mod server {
        use super::super::*;
        use std::io::{Read as _, Write as _};
        use std::net::{TcpListener, TcpStream};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        pub(crate) type Handler = Box<dyn FnOnce(TcpStream) + Send>;

        /// Loopback server answering one connection per handler, in order.
        pub(crate) struct Stub {
            pub port: u16,
            /// Request heads received, in order.
            pub heads: Arc<Mutex<Vec<String>>>,
            thread: Option<std::thread::JoinHandle<()>>,
        }

        impl Stub {
            pub(crate) fn start(handlers: Vec<Handler>) -> Self {
                let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback stub");
                Self::serve(listener, handlers)
            }

            pub(crate) fn serve(listener: TcpListener, handlers: Vec<Handler>) -> Self {
                let port = listener.local_addr().unwrap().port();
                let heads = Arc::new(Mutex::new(Vec::new()));
                let recorded = Arc::clone(&heads);
                let thread = std::thread::spawn(move || {
                    for handler in handlers {
                        let Ok((mut stream, _)) = listener.accept() else {
                            return;
                        };
                        let head = read_request_head(&mut stream);
                        recorded
                            .lock()
                            .unwrap()
                            .push(String::from_utf8_lossy(&head).into_owned());
                        handler(stream);
                    }
                });
                Self {
                    port,
                    heads,
                    thread: Some(thread),
                }
            }

            pub(crate) fn url(&self, path: &str) -> String {
                format!("http://127.0.0.1:{}{path}", self.port)
            }

            pub(crate) fn authority(&self) -> String {
                format!("127.0.0.1:{}", self.port)
            }

            pub(crate) fn join(mut self) {
                if let Some(thread) = self.thread.take() {
                    thread.join().expect("stub thread");
                }
            }
        }

        fn read_request_head(stream: &mut TcpStream) -> Vec<u8> {
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(1) => head.push(byte[0]),
                    _ => break,
                }
            }
            head
        }

        pub(crate) fn respond(raw: Vec<u8>) -> Handler {
            Box::new(move |mut stream| {
                let _ = stream.write_all(&raw);
            })
        }

        pub(crate) fn ok_with_length(body: Vec<u8>) -> Handler {
            let mut raw = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes();
            raw.extend(body);
            respond(raw)
        }

        pub(crate) fn ok_close_delimited(body: Vec<u8>) -> Handler {
            let mut raw = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
            raw.extend(body);
            respond(raw)
        }

        pub(crate) fn redirect(status: u16, location: &str) -> Handler {
            respond(
                format!(
                    "HTTP/1.1 {status} Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .into_bytes(),
            )
        }

        fn chunked(body: &[u8], chunk: usize) -> Vec<u8> {
            let mut raw =
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                    .to_vec();
            for part in body.chunks(chunk) {
                raw.extend(format!("{:x}\r\n", part.len()).into_bytes());
                raw.extend(part);
                raw.extend(b"\r\n");
            }
            raw.extend(b"0\r\n\r\n");
            raw
        }

        fn local(stub: &Stub) -> UrlPolicy {
            UrlPolicy::exact_hosts(&[]).with_local_authority(&stub.authority())
        }

        fn fetch(stub: &Stub, path: &str, max: u64) -> Result<FetchedBody, DownloadError> {
            fetch_bounded_with_timeout(
                &local(stub),
                &stub.url(path),
                max,
                Duration::from_secs(10),
                UPSTREAM_HEADERS,
            )
        }

        fn body(len: usize) -> Vec<u8> {
            (0..len).map(|i| (i % 251) as u8).collect()
        }

        #[test]
        fn the_given_request_headers_are_sent() {
            let stub = Stub::start(vec![ok_with_length(b"ok".to_vec())]);
            fetch(&stub, "/", 10).unwrap();
            let head = stub.heads.lock().unwrap()[0].to_ascii_lowercase();
            assert!(head.contains("user-agent: d4companion\r\n"), "{head}");
            assert!(head.contains("cache-control: no-cache\r\n"), "{head}");
            stub.join();
        }

        #[test]
        fn exactly_three_redirects_are_followed_and_a_fourth_is_refused() {
            let stub = Stub::start(vec![
                redirect(302, "/1"),
                redirect(302, "/2"),
                redirect(302, "/3"),
                ok_with_length(b"ok".to_vec()),
                redirect(302, "/a"),
                redirect(302, "/b"),
                redirect(302, "/c"),
                redirect(302, "/d"),
            ]);
            assert_eq!(fetch(&stub, "/0", 10).unwrap().bytes, b"ok");
            assert!(matches!(
                fetch(&stub, "/x", 10),
                Err(DownloadError::RedirectRefused(_))
            ));
            stub.join();
        }

        #[test]
        fn relative_locations_resolve_and_protocol_relative_ones_are_checked() {
            let stub = Stub::start(vec![
                redirect(302, "../x/./y"),
                ok_with_length(b"ok".to_vec()),
                redirect(302, "//localhost:1/x"),
            ]);
            let fetched = fetch(&stub, "/a/b/c", 10).unwrap();
            assert_eq!(fetched.final_url, stub.url("/a/x/y"));
            assert!(matches!(
                fetch(&stub, "/", 10),
                Err(DownloadError::RedirectRefused(_))
            ));
            stub.join();
        }

        /// gzip member holding `data` in one stored deflate block.
        fn gzip_stored(data: &[u8]) -> Vec<u8> {
            let len = u16::try_from(data.len()).unwrap();
            let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255, 1];
            out.extend(len.to_le_bytes());
            out.extend((!len).to_le_bytes());
            out.extend(data);
            let mut crc = 0xffff_ffff_u32;
            for &byte in data {
                crc ^= u32::from(byte);
                for _ in 0..8 {
                    crc = if crc & 1 != 0 {
                        (crc >> 1) ^ 0xedb8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            out.extend((!crc).to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            out
        }

        #[test]
        fn gzip_bodies_are_capped_after_decoding_and_truncation_is_an_error() {
            let gzip_head = |extra: &str| {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\n{extra}Connection: close\r\n\r\n"
                )
                .into_bytes()
            };
            let small = gzip_stored(&body(1000));
            let mut whole = gzip_head("");
            whole.extend(&small);
            let mut cut = gzip_head("");
            cut.extend(&small[..small.len() - 300]);
            // Encoded length within the cap, decoded length above it.
            let big = gzip_stored(&[7_u8; 60_000]);
            let mut bomb = gzip_head(&format!("Content-Length: {}\r\n", big.len()));
            bomb.extend(&big);
            let stub = Stub::start(vec![respond(whole), respond(cut), respond(bomb)]);
            assert_eq!(fetch(&stub, "/", 5000).unwrap().bytes, body(1000));
            assert!(matches!(
                fetch(&stub, "/", 5000),
                Err(DownloadError::Transport(_))
            ));
            assert!(matches!(
                fetch(&stub, "/", 50_000),
                Err(DownloadError::BodyTooLarge { declared: None, .. })
            ));
            stub.join();
        }

        #[test]
        fn small_and_exactly_max_bodies_are_returned_whole() {
            let stub = Stub::start(vec![
                ok_with_length(body(10)),
                ok_with_length(body(1000)),
                ok_close_delimited(body(1000)),
                respond(chunked(&body(1000), 97)),
            ]);
            assert_eq!(fetch(&stub, "/", 1000).unwrap().bytes, body(10));
            for _ in 0..3 {
                let fetched = fetch(&stub, "/", 1000).unwrap();
                assert_eq!(fetched.bytes, body(1000));
                assert_eq!(fetched.final_url, stub.url("/"));
            }
            stub.join();
        }

        #[test]
        fn max_plus_one_is_refused_with_or_without_content_length() {
            let stub = Stub::start(vec![
                ok_with_length(body(1001)),
                ok_close_delimited(body(1001)),
                respond(chunked(&body(5000), 512)),
            ]);
            assert!(matches!(
                fetch(&stub, "/", 1000),
                Err(DownloadError::BodyTooLarge {
                    limit: 1000,
                    declared: Some(1001)
                })
            ));
            assert!(matches!(
                fetch(&stub, "/", 1000),
                Err(DownloadError::BodyTooLarge {
                    limit: 1000,
                    declared: None
                })
            ));
            assert!(matches!(
                fetch(&stub, "/", 1000),
                Err(DownloadError::BodyTooLarge { declared: None, .. })
            ));
            stub.join();
        }

        #[test]
        fn a_huge_body_without_length_stops_reading_at_the_cap() {
            // 64 MiB offered, 1 MiB cap: the client stops after cap + 1 bytes.
            let stub = Stub::start(vec![Box::new(|mut stream: TcpStream| {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
                let block = vec![0_u8; 64 * 1024];
                for _ in 0..1024 {
                    if stream.write_all(&block).is_err() {
                        return;
                    }
                }
            })]);
            assert!(matches!(
                fetch(&stub, "/", 1024 * 1024),
                Err(DownloadError::BodyTooLarge { .. })
            ));
            stub.join();
        }

        #[test]
        fn a_body_shorter_than_its_content_length_is_an_error() {
            let stub = Stub::start(vec![respond(
                b"HTTP/1.1 200 OK\r\nContent-Length: 500\r\nConnection: close\r\n\r\nonly-this"
                    .to_vec(),
            )]);
            let error = fetch(&stub, "/", 1000).unwrap_err();
            assert!(matches!(error, DownloadError::Transport(_)), "{error}");
            stub.join();
        }

        #[test]
        fn a_content_length_smaller_than_the_body_yields_exactly_the_declared_bytes() {
            let stub = Stub::start(vec![respond(
                b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPK\x03\x04trailing-garbage"
                    .to_vec(),
            )]);
            assert_eq!(fetch(&stub, "/", 1000).unwrap().bytes, b"PK\x03\x04");
            stub.join();
        }

        #[test]
        fn an_aborted_connection_is_an_error_never_a_complete_body() {
            let stub = Stub::start(vec![
                Box::new(|mut stream: TcpStream| {
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\npartial",
                    );
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }),
                Box::new(|mut stream: TcpStream| {
                    // Cut inside the second chunk's size line.
                    let raw = chunked(&body(300), 100);
                    let _ = stream.write_all(&raw[..raw.len() - 217]);
                    let _ = stream.write_all(b"6");
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }),
                Box::new(|mut stream: TcpStream| {
                    // Cut inside chunk data.
                    let mut raw = chunked(&body(300), 100);
                    raw.truncate(raw.len() - 120);
                    let _ = stream.write_all(&raw);
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }),
            ]);
            for case in ["content-length", "chunk header"] {
                let error = fetch(&stub, "/", 1000).unwrap_err();
                assert!(
                    matches!(error, DownloadError::Transport(_)),
                    "{case}: {error}"
                );
            }
            // ureq 2.12's chunk decoder reports EOF inside chunk data as the
            // end of the body: the caller gets a strict prefix, never the
            // whole body, and must validate the content.
            let prefix = fetch(&stub, "/", 1000).unwrap().bytes;
            assert!(prefix.len() < 300 && body(300).starts_with(&prefix));
            stub.join();
        }

        #[test]
        fn error_bodies_are_kept_but_capped() {
            let big = body(10 * 1024 * 1024);
            let mut raw = format!(
                "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                big.len()
            )
            .into_bytes();
            raw.extend(&big);
            let stub = Stub::start(vec![
                respond(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 14\r\nConnection: close\r\n\r\n404: Not Found"
                        .to_vec(),
                ),
                respond(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec(),
                ),
                respond(raw),
            ]);
            match fetch(&stub, "/", 1000) {
                Err(DownloadError::HttpStatus {
                    status: 404, body, ..
                }) => {
                    assert_eq!(body, b"404: Not Found");
                }
                other => panic!("{other:?}"),
            }
            match fetch(&stub, "/", 1000) {
                Err(DownloadError::HttpStatus {
                    status: 503, body, ..
                }) => assert!(body.is_empty()),
                other => panic!("{other:?}"),
            }
            match fetch(&stub, "/", 1000) {
                Err(DownloadError::HttpStatus {
                    status: 500, body, ..
                }) => {
                    assert_eq!(body.len() as u64, MAX_ERROR_BODY_BYTES);
                    assert_eq!(body[..], big[..body.len()]);
                }
                other => panic!("{other:?}"),
            }
            stub.join();
        }

        /// Runs `fetch` on its own thread so a timeout regression fails the
        /// assert instead of hanging the suite.
        fn fetch_with_watchdog(
            policy: UrlPolicy,
            url: String,
            timeout: Duration,
        ) -> Result<FetchedBody, DownloadError> {
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(fetch_bounded_with_timeout(
                    &policy,
                    &url,
                    1000,
                    timeout,
                    UPSTREAM_HEADERS,
                ));
            });
            receiver
                .recv_timeout(Duration::from_secs(15))
                .expect("the fetch hung past 15 s; the deadline did not fire")
        }

        #[test]
        fn a_stalled_or_dripping_server_hits_the_deadline() {
            let stalled = Stub::start(vec![Box::new(|stream: TcpStream| {
                std::thread::sleep(Duration::from_secs(4));
                drop(stream);
            })]);
            let dripping = Stub::start(vec![Box::new(|mut stream: TcpStream| {
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n",
                );
                for _ in 0..40 {
                    if stream.write_all(b"x").is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })]);
            for (case, stub) in [("stalled", &stalled), ("dripping", &dripping)] {
                let started = std::time::Instant::now();
                let error = fetch_with_watchdog(local(stub), stub.url("/"), Duration::from_secs(1))
                    .unwrap_err();
                let message = error.to_string().to_lowercase();
                assert!(
                    matches!(error, DownloadError::Transport(_))
                        && (message.contains("timed out") || message.contains("timeout")),
                    "{case}: {message}"
                );
                assert!(started.elapsed() < Duration::from_secs(3), "{case}");
            }
            stalled.join();
            dripping.join();
        }

        #[test]
        fn the_production_timeout_mirrors_the_upstream_httpclient_default() {
            assert_eq!(DOWNLOAD_HTTP_TIMEOUT, Duration::from_secs(100));
        }

        #[test]
        fn an_allowed_redirect_chain_is_followed_to_the_final_body() {
            let second = Stub::start(vec![ok_with_length(b"final".to_vec())]);
            let first = Stub::start(vec![
                redirect(301, "/hop"),
                redirect(302, &second.url("/raw/a.zip")),
            ]);
            let policy = local(&first).with_local_authority(&second.authority());
            let fetched = fetch_bounded_with_timeout(
                &policy,
                &first.url("/a.zip"),
                100,
                Duration::from_secs(10),
                UPSTREAM_HEADERS,
            )
            .unwrap();
            assert_eq!(fetched.bytes, b"final");
            assert_eq!(fetched.final_url, second.url("/raw/a.zip"));
            first.join();
            second.join();
        }

        #[test]
        fn redirect_loops_and_long_chains_are_refused() {
            let stub = Stub::start(vec![
                redirect(302, "/b"),
                redirect(302, "/a"),
                redirect(302, "/2"),
                redirect(302, "/3"),
                redirect(302, "/4"),
                redirect(302, "/5"),
            ]);
            let error = fetch(&stub, "/a", 100).unwrap_err();
            assert!(
                matches!(&error, DownloadError::RedirectRefused(m) if m.contains("loop")),
                "{error}"
            );
            let error = fetch(&stub, "/1", 100).unwrap_err();
            assert!(
                matches!(&error, DownloadError::RedirectRefused(m) if m.contains("more than")),
                "{error}"
            );
            stub.join();
        }

        #[test]
        fn redirects_to_other_ports_hosts_userinfo_or_http_are_refused_before_connecting() {
            // A listener on another port that must never see a connection.
            let bystander = TcpListener::bind("127.0.0.1:0").unwrap();
            bystander.set_nonblocking(true).unwrap();
            let other_port = bystander.local_addr().unwrap().port();
            let targets = [
                format!("http://127.0.0.1:{other_port}/x"),
                format!("http://localhost:{other_port}/x"),
                "https://example.com/x".to_string(),
                "http://github.com/x".to_string(),
                "https://192.168.1.1/x".to_string(),
                "ftp://github.com/x".to_string(),
                String::new(),
            ];
            let mut handlers: Vec<Handler> = Vec::new();
            let stub_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = stub_listener.local_addr().unwrap().port();
            let userinfo = format!("http://user:pw@127.0.0.1:{port}/x");
            for target in targets.iter().chain(std::iter::once(&userinfo)) {
                handlers.push(if target.is_empty() {
                    respond(
                        b"HTTP/1.1 302 Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec(),
                    )
                } else {
                    redirect(302, target)
                });
            }
            let stub = Stub::serve(stub_listener, handlers);
            let policy =
                UrlPolicy::exact_hosts(&["github.com"]).with_local_authority(&stub.authority());
            for target in targets.iter().chain(std::iter::once(&userinfo)) {
                let error = fetch_bounded_with_timeout(
                    &policy,
                    &stub.url("/start"),
                    100,
                    Duration::from_secs(10),
                    UPSTREAM_HEADERS,
                )
                .unwrap_err();
                assert!(
                    matches!(error, DownloadError::RedirectRefused(_)),
                    "{target}: {error}"
                );
            }
            stub.join();
            assert!(
                bystander.accept().is_err(),
                "a refused redirect target must never be contacted"
            );
        }
    }
}
