use crate::models::{
    Allowance, AllowanceFull, AllowanceRemaining, ImportPreview, NodeView, ParsedNode, PreviewNode,
    SourceType,
};
use anyhow::{Context, Result, bail};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use regex::Regex;
use serde_yaml_ng::{Mapping, Value};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::OnceLock,
};
use url::{Url, form_urlencoded};

const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_PROVIDERS: usize = 12;
const MAX_REDIRECTS: usize = 4;
const SUPPORTED_TYPES: [&str; 4] = ["trojan", "ss", "vmess", "vless"];
const SUBSCRIPTION_ACCEPT: &str =
    "application/yaml, application/x-yaml, text/yaml, text/plain;q=0.9, */*;q=0.8";

#[derive(Debug)]
enum SubscriptionDocument {
    Profile(Mapping),
    Proxies(Vec<Mapping>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocumentShape {
    ProfileMapping,
    BareProxyList,
    ShareLinkLines,
    Base64Wrapped,
}

impl DocumentShape {
    fn label(self) -> &'static str {
        match self {
            Self::ProfileMapping => "profile-mapping",
            Self::BareProxyList => "bare-proxy-list",
            Self::ShareLinkLines => "share-link-list",
            Self::Base64Wrapped => "base64-wrapped-list",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ResponseSummary {
    status: u16,
    redirects: usize,
    media_type: &'static str,
    encoding: &'static str,
    decoded_body_bytes: usize,
}

#[derive(Debug, Clone, Copy)]
struct FetchSummary {
    response: ResponseSummary,
    shape: DocumentShape,
    supported_nodes: usize,
    unsupported_rows: usize,
    notice_rows: usize,
}

struct FetchedResponse {
    body: String,
    headers: Vec<(String, String)>,
    summary: ResponseSummary,
}

#[derive(Clone)]
pub struct ParsedSource {
    pub title: String,
    pub source_host: Option<String>,
    pub nodes: Vec<ParsedNode>,
    pub unsupported_count: usize,
    pub notice_count: usize,
    pub allowance: Allowance,
    fetch_summary: Option<FetchSummary>,
}

pub(crate) fn subscription_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("Clashfork/1.0 (Windows)")
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to initialize the subscription HTTP client")
}

impl ParsedSource {
    pub fn preview(&self, id: String, source_type: SourceType) -> ImportPreview {
        ImportPreview {
            id,
            source_type,
            title: self.title.clone(),
            source_host: self.source_host.clone(),
            node_count: self.nodes.len(),
            unsupported_count: self.unsupported_count,
            notice_count: self.notice_count,
            allowance: self.allowance.clone(),
            stale: false,
            nodes: self
                .nodes
                .iter()
                .take(500)
                .map(|node| PreviewNode {
                    display_name: node.display_name.clone(),
                    protocol: node.protocol.clone(),
                })
                .collect(),
        }
    }
}

pub fn source_type_for_input(input: &str) -> Result<SourceType> {
    let input = input.trim();
    if input.to_ascii_lowercase().starts_with("https://") {
        return Ok(SourceType::Subscription);
    }
    let lower = input.to_ascii_lowercase();
    if ["trojan://", "ss://", "vmess://", "vless://"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        return Ok(SourceType::Link);
    }
    if lower.starts_with("http://") {
        bail!("Only HTTPS subscription URLs are accepted.");
    }
    bail!("Enter an HTTPS Clash/Mihomo subscription URL or a supported share link.")
}

pub async fn fetch_source(client: &reqwest::Client, input: &str) -> Result<ParsedSource> {
    fetch_source_with_host_policy(client, input, None).await
}

#[cfg(test)]
async fn fetch_source_on_host(
    client: &reqwest::Client,
    input: &str,
    allowed_host: &str,
) -> Result<ParsedSource> {
    fetch_source_with_host_policy(client, input, Some(allowed_host)).await
}

async fn fetch_source_with_host_policy(
    client: &reqwest::Client,
    input: &str,
    allowed_host: Option<&str>,
) -> Result<ParsedSource> {
    let input = input.trim();
    let source_type = source_type_for_input(input)?;
    match source_type {
        SourceType::Subscription => fetch_subscription(client, input, allowed_host).await,
        SourceType::Link => {
            let node = parse_share_link(input)?;
            let title = node.display_name.clone();
            Ok(ParsedSource {
                title,
                source_host: None,
                nodes: vec![node],
                unsupported_count: 0,
                notice_count: 0,
                allowance: Allowance::Unavailable,
                fetch_summary: None,
            })
        }
        SourceType::Manual => unreachable!("manual sources are not fetched"),
    }
}

async fn fetch_subscription(
    client: &reqwest::Client,
    input: &str,
    allowed_host: Option<&str>,
) -> Result<ParsedSource> {
    let url = Url::parse(input).context("The subscription URL is invalid.")?;
    if url.scheme() != "https" {
        bail!("Only HTTPS subscription URLs are accepted.");
    }
    if allowed_host.is_some_and(|host| {
        !url.host_str()
            .is_some_and(|actual| actual.eq_ignore_ascii_case(host))
    }) {
        bail!("The configured subscription host does not match the permitted provider host.");
    }
    let host = url.host_str().map(str::to_owned);
    let response = fetch_https(client, &url, None, allowed_host).await?;
    let (document, shape) = parse_response_document(&response.body, response.summary.media_type)
        .map_err(|error| format_response_parse_error(error, response.summary))?;
    let mut raw_proxies = match &document {
        SubscriptionDocument::Profile(root) => {
            let mut proxies = Vec::new();
            if let Some(value) = root.get(Value::String("proxies".into())) {
                proxies.extend(
                    mapping_sequence(value, "The subscription's proxies field is invalid.")
                        .map_err(|error| format_response_parse_error(error, response.summary))?,
                );
            }
            proxies
        }
        SubscriptionDocument::Proxies(proxies) => proxies.clone(),
    };

    let mut unsupported_provider_count = 0usize;
    if let SubscriptionDocument::Profile(root) = &document
        && let Some(providers) = root.get(Value::String("proxy-providers".into()))
    {
        let Some(provider_map) = providers.as_mapping() else {
            bail!("The subscription's proxy-providers field is invalid.");
        };
        for (index, (_name, provider)) in provider_map.iter().enumerate() {
            if index >= MAX_PROVIDERS {
                unsupported_provider_count += 1;
                continue;
            }
            let Some(provider) = provider.as_mapping() else {
                unsupported_provider_count += 1;
                continue;
            };
            if provider
                .get(Value::String("type".into()))
                .and_then(Value::as_str)
                != Some("http")
            {
                unsupported_provider_count += 1;
                continue;
            }
            let Some(raw_url) = provider
                .get(Value::String("url".into()))
                .and_then(Value::as_str)
            else {
                unsupported_provider_count += 1;
                continue;
            };
            let provider_url = Url::parse(raw_url)
                .map_err(|_| anyhow::anyhow!("A node provider has an invalid HTTPS source."))?;
            if provider_url.scheme() != "https" {
                bail!("Node provider sources must use HTTPS.");
            }
            if allowed_host.is_some_and(|host| {
                !provider_url
                    .host_str()
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(host))
            }) {
                bail!("An embedded node provider points outside the configured provider host.");
            }
            let provider_headers = parse_provider_headers(provider)?;
            let provider_response =
                fetch_https(client, &provider_url, Some(&provider_headers), allowed_host).await?;
            let provider_document = parse_response_document(
                &provider_response.body,
                provider_response.summary.media_type,
            )
            .context("A node provider did not return a supported proxy list.")?
            .0;
            let provider_proxies = match provider_document {
                SubscriptionDocument::Profile(profile) => profile
                    .get(Value::String("proxies".into()))
                    .context("A node provider did not contain a proxies list.")
                    .and_then(|value| {
                        mapping_sequence(value, "A node provider's proxies field is invalid.")
                    })?,
                SubscriptionDocument::Proxies(proxies) => proxies,
            };
            raw_proxies.extend(provider_proxies);
        }
    }

    let title = service_title(&response.headers, host.as_deref());
    let allowance = choose_allowance(&response.headers, &raw_proxies);
    let (nodes, notice_count, mut unsupported_count) = filter_and_sanitize_nodes(raw_proxies)
        .map_err(|error| format_response_parse_error(error, response.summary))?;
    unsupported_count += unsupported_provider_count;
    if nodes.is_empty() {
        return Err(format_response_parse_error(
            anyhow::anyhow!(
                "The subscription contained no supported Trojan, Shadowsocks, VMess, or VLESS nodes."
            ),
            response.summary,
        ));
    }
    let supported_nodes = nodes.len();
    Ok(ParsedSource {
        title,
        source_host: host,
        nodes,
        unsupported_count,
        notice_count,
        allowance,
        fetch_summary: Some(FetchSummary {
            response: response.summary,
            shape,
            supported_nodes,
            unsupported_rows: unsupported_count,
            notice_rows: notice_count,
        }),
    })
}

async fn fetch_https(
    client: &reqwest::Client,
    url: &Url,
    provider_headers: Option<&reqwest::header::HeaderMap>,
    allowed_host: Option<&str>,
) -> Result<FetchedResponse> {
    if url.scheme() != "https" {
        bail!("Only HTTPS sources are accepted.");
    }
    let mut current_url = url.clone();
    let mut current_headers = provider_headers.cloned();
    let mut redirects_followed = 0usize;
    let response = loop {
        if allowed_host.is_some_and(|host| {
            !current_url
                .host_str()
                .is_some_and(|actual| actual.eq_ignore_ascii_case(host))
        }) {
            bail!(
                "The subscription request attempted to contact a host outside the configured provider."
            );
        }
        let mut request = subscription_request(client, &current_url);
        if let Some(headers) = &current_headers {
            request = request.headers(headers.clone());
        }
        let response = request
            .send()
            .await
            .map_err(|error| safe_request_error(&error))?;
        if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            break response;
        }
        if redirects_followed >= MAX_REDIRECTS {
            bail!("The subscription redirected too many times.");
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .context("The subscription redirect did not contain a valid location.")?;
        let next_url = resolve_https_redirect(&current_url, location)?;
        if allowed_host.is_some_and(|host| {
            !next_url
                .host_str()
                .is_some_and(|actual| actual.eq_ignore_ascii_case(host))
        }) {
            bail!("The subscription redirected outside the configured provider host.");
        }
        if !same_origin(&current_url, &next_url) {
            current_headers = None;
        }
        current_url = next_url;
        redirects_followed += 1;
    };
    if !response.status().is_success() {
        bail!(
            "The subscription server returned HTTP {}.",
            response.status().as_u16()
        );
    }
    if let Some(length) = response.content_length()
        && length > MAX_BODY_BYTES as u64
    {
        bail!("The subscription response is larger than the 4 MB safety limit.");
    }
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), value.to_owned()))
        })
        .collect::<Vec<_>>();
    let mut summary = ResponseSummary {
        status: response.status().as_u16(),
        redirects: redirects_followed,
        media_type: response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(safe_media_type)
            .unwrap_or("missing"),
        encoding: response
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .map(safe_content_encoding)
            .unwrap_or("missing-or-decoded"),
        decoded_body_bytes: 0,
    };
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    use futures_util::StreamExt as _;
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| anyhow::anyhow!("The subscription download was interrupted."))?;
        if body.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            bail!("The subscription response is larger than the 4 MB safety limit.");
        }
        body.extend_from_slice(&chunk);
    }
    summary.decoded_body_bytes = body.len();
    let body = String::from_utf8(body).context("The subscription response was not valid UTF-8.")?;
    Ok(FetchedResponse {
        body,
        headers,
        summary,
    })
}

fn safe_media_type(raw: &str) -> &'static str {
    let value = raw.split(';').next().unwrap_or_default().trim();
    if value.eq_ignore_ascii_case("application/yaml")
        || value.eq_ignore_ascii_case("application/x-yaml")
        || value.eq_ignore_ascii_case("text/yaml")
    {
        "yaml"
    } else if value.eq_ignore_ascii_case("text/plain") {
        "plain-text"
    } else if value.eq_ignore_ascii_case("text/html") {
        "html"
    } else if value.eq_ignore_ascii_case("application/octet-stream") {
        "binary"
    } else if value.is_empty() {
        "missing"
    } else {
        "other"
    }
}

fn safe_content_encoding(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "identity" => "identity",
        "gzip" => "gzip",
        "br" => "br",
        "deflate" => "deflate",
        "zstd" => "zstd",
        _ => "other",
    }
}

fn format_response_parse_error(error: anyhow::Error, response: ResponseSummary) -> anyhow::Error {
    let message = error.to_string();
    anyhow::anyhow!(
        "{message} (HTTP {}; media type {}; encoding {}; decoded body {} bytes; {} redirects).",
        response.status,
        response.media_type,
        response.encoding,
        response.decoded_body_bytes,
        response.redirects
    )
}

fn subscription_request(client: &reqwest::Client, url: &Url) -> reqwest::RequestBuilder {
    client
        .get(url.clone())
        .header(
            reqwest::header::USER_AGENT,
            format!("clash-verge/v{}", env!("CARGO_PKG_VERSION")),
        )
        .header(reqwest::header::ACCEPT, SUBSCRIPTION_ACCEPT)
}

fn parse_provider_headers(provider: &Mapping) -> Result<reqwest::header::HeaderMap> {
    let mut result = reqwest::header::HeaderMap::new();
    let Some(value) = provider.get(Value::String("header".into())) else {
        return Ok(result);
    };
    let Some(headers) = value.as_mapping() else {
        bail!("A node provider's headers are invalid.");
    };
    for (name, values) in headers {
        let Some(name) = name.as_str() else {
            bail!("A node provider's headers are invalid.");
        };
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| anyhow::anyhow!("A node provider's headers are invalid."))?;
        let values: Vec<&str> = if let Some(value) = values.as_str() {
            vec![value]
        } else if let Some(values) = values.as_sequence() {
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .context("A node provider's headers are invalid.")
                })
                .collect::<Result<_>>()?
        } else {
            bail!("A node provider's headers are invalid.");
        };
        for value in values {
            let mut value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| anyhow::anyhow!("A node provider's headers are invalid."))?;
            value.set_sensitive(true);
            result.append(name.clone(), value);
        }
    }
    Ok(result)
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn resolve_https_redirect(current: &Url, location: &str) -> Result<Url> {
    let next = current
        .join(location)
        .map_err(|_| anyhow::anyhow!("The subscription redirect URL is invalid."))?;
    if next.scheme() != "https" {
        bail!("The subscription redirected outside HTTPS.");
    }
    Ok(next)
}

fn parse_subscription_document(body: &str) -> Result<SubscriptionDocument> {
    parse_response_document(body, "missing").map(|(document, _)| document)
}

fn parse_response_document(
    body: &str,
    _negotiated_media_type: &str,
) -> Result<(SubscriptionDocument, DocumentShape)> {
    // Providers sometimes label a valid Clash profile as text/html. The body shape is
    // authoritative; the media type is retained for safe diagnostics only.
    parse_subscription_document_inner(body, true)
}

fn parse_subscription_document_inner(
    body: &str,
    allow_base64: bool,
) -> Result<(SubscriptionDocument, DocumentShape)> {
    let body = body.trim_start_matches('\u{feff}').trim();
    if body.is_empty() {
        bail!("The subscription response was empty.");
    }
    if looks_like_html_error_page(body) {
        bail!(
            "The subscription server returned an HTML/error page instead of a Clash/Mihomo profile."
        );
    }

    if has_share_link_line(body) {
        return parse_share_link_lines(body)
            .map(SubscriptionDocument::Proxies)
            .map(|document| (document, DocumentShape::ShareLinkLines));
    }

    if let Ok(value) = serde_yaml_ng::from_str::<Value>(body) {
        match value {
            Value::Mapping(root) => {
                return Ok((
                    SubscriptionDocument::Profile(root),
                    DocumentShape::ProfileMapping,
                ));
            }
            Value::Sequence(items) => {
                if items.iter().all(|item| item.as_mapping().is_some()) {
                    return Ok((
                        SubscriptionDocument::Proxies(
                            items
                                .iter()
                                .filter_map(|item| item.as_mapping().cloned())
                                .collect(),
                        ),
                        DocumentShape::BareProxyList,
                    ));
                }
                if items.iter().all(|item| item.as_str().is_some()) {
                    let lines = items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n");
                    if has_share_link_line(&lines) {
                        return parse_share_link_lines(&lines)
                            .map(SubscriptionDocument::Proxies)
                            .map(|document| (document, DocumentShape::ShareLinkLines));
                    }
                }
            }
            Value::String(_) | Value::Bool(_) | Value::Number(_) | Value::Null => {}
            _ => {}
        }
    }

    if allow_base64 && let Some(decoded) = decode_base64_subscription(body) {
        let decoded = String::from_utf8(decoded)
            .context("The encoded subscription did not contain valid UTF-8 text.")?;
        return parse_subscription_document_inner(&decoded, false)
            .map(|(document, _)| (document, DocumentShape::Base64Wrapped));
    }

    bail!("The subscription response is not a supported Clash/Mihomo profile or node list.")
}

fn looks_like_html_error_page(body: &str) -> bool {
    let prefix = body
        .trim_start_matches('\u{feff}')
        .trim_start()
        .chars()
        .take(512)
        .collect::<String>()
        .to_ascii_lowercase();
    ["<!doctype html", "<html", "<head", "<body", "<form"]
        .iter()
        .any(|marker| {
            prefix.starts_with(marker) || prefix.starts_with("<?xml") && prefix.contains(marker)
        })
}

fn mapping_sequence(value: &Value, error: &str) -> Result<Vec<Mapping>> {
    let Some(sequence) = value.as_sequence() else {
        bail!("{error}");
    };
    sequence
        .iter()
        .map(|item| {
            item.as_mapping()
                .cloned()
                .context("The subscription contains a malformed proxy entry.")
        })
        .collect()
}

fn has_share_link_line(body: &str) -> bool {
    body.lines().any(|line| {
        let line = line.trim_start_matches('\u{feff}').trim();
        !line.is_empty()
            && !line.starts_with('#')
            && ["trojan://", "ss://", "vmess://", "vless://"]
                .iter()
                .any(|prefix| line.to_ascii_lowercase().starts_with(prefix))
    })
}

fn parse_share_link_lines(body: &str) -> Result<Vec<Mapping>> {
    let mut proxies = Vec::new();
    for line in body.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') || is_notice_label(line) {
            continue;
        }
        let link = line.to_ascii_lowercase();
        if !["trojan://", "ss://", "vmess://", "vless://"]
            .iter()
            .any(|prefix| link.starts_with(prefix))
        {
            bail!("The subscription contains an invalid share-link line.");
        }
        let parsed =
            parse_share_link(line).context("The subscription contains an invalid share link.")?;
        let Some(proxy) = parsed.proxy.as_mapping() else {
            bail!("The subscription contains an invalid share link.");
        };
        proxies.push(proxy.clone());
    }
    if proxies.is_empty() {
        bail!("The subscription contained no supported share links.");
    }
    Ok(proxies)
}

fn decode_base64_subscription(body: &str) -> Option<Vec<u8>> {
    let compact = body
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>();
    if compact.len() < 16 || compact.len() > MAX_BODY_BYTES.saturating_mul(4) / 3 + 16 {
        return None;
    }
    let decoded = STANDARD
        .decode(&compact)
        .or_else(|_| STANDARD_NO_PAD.decode(&compact))
        .or_else(|_| URL_SAFE.decode(&compact))
        .or_else(|_| URL_SAFE_NO_PAD.decode(&compact))
        .ok()?;
    (decoded.len() <= MAX_BODY_BYTES).then_some(decoded)
}

fn safe_request_error(error: &reqwest::Error) -> anyhow::Error {
    if error.is_timeout() {
        anyhow::anyhow!("The subscription request timed out.")
    } else if error.is_connect() {
        anyhow::anyhow!("The subscription server could not be reached over TLS.")
    } else if error.is_redirect() {
        anyhow::anyhow!("The subscription redirected outside HTTPS or too many times.")
    } else if error.is_body() || error.is_decode() {
        anyhow::anyhow!("The subscription response could not be read safely.")
    } else {
        anyhow::anyhow!("The subscription request failed.")
    }
}

pub fn service_title(headers: &[(String, String)], host: Option<&str>) -> String {
    for (name, value) in headers {
        match name.to_ascii_lowercase().as_str() {
            "profile-title" => {
                if let Some(title) = decode_title(value) {
                    return title;
                }
            }
            "content-disposition" => {
                if let Some(title) = filename_title(value) {
                    return title;
                }
            }
            _ => {}
        }
    }
    if let Some(host) = host {
        let clean = sanitize_title(host);
        if !clean.is_empty() {
            return clean;
        }
    }
    "Subscription".to_owned()
}

fn decode_title(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some(encoded) = raw.strip_prefix("base64:") {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .ok()?;
        let title = sanitize_title(&String::from_utf8_lossy(&decoded));
        return (!title.is_empty()).then_some(title);
    }
    let title = sanitize_title(raw);
    (!title.is_empty()).then_some(title)
}

fn filename_title(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let marker = "filename*=";
    let raw = if let Some(index) = lower.find(marker) {
        value[index + marker.len()..]
            .split(';')
            .next()?
            .trim()
            .split("''")
            .nth(1)
            .unwrap_or("")
    } else {
        let marker = "filename=";
        let index = lower.find(marker)?;
        value[index + marker.len()..]
            .split(';')
            .next()?
            .trim()
            .trim_matches('"')
    };
    let raw = percent_encoding::percent_decode_str(raw).decode_utf8_lossy();
    let stem = std::path::Path::new(raw.as_ref())
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())?;
    let title = sanitize_title(&stem);
    (!title.is_empty()).then_some(title)
}

fn sanitize_title(raw: &str) -> String {
    let no_url = Regex::new(r"(?i)[a-z][a-z0-9+.-]*://\S+").expect("title regex");
    let raw = no_url.replace_all(raw, "").into_owned();
    let mut clean = String::new();
    for character in raw.chars() {
        if clean.chars().count() >= 64 {
            break;
        }
        if character.is_control() || matches!(character, '/' | '\\' | '\r' | '\n' | '\t') {
            continue;
        }
        clean.push(character);
    }
    let clean = clean.trim().trim_matches('"').trim().to_owned();
    if clean.contains('?') || clean.contains('#') {
        return "Subscription".to_owned();
    }
    clean
}

fn header_for_subscription_userinfo(name: &str) -> Option<bool> {
    let name = name.to_ascii_lowercase();
    if name == "subscription-userinfo" {
        Some(true)
    } else if name.ends_with("subscription-userinfo") {
        let prefix = &name[..name.len() - "subscription-userinfo".len()];
        if prefix.ends_with('-') || prefix.ends_with('_') {
            return Some(false);
        }
        None
    } else {
        None
    }
}

pub fn choose_allowance(headers: &[(String, String)], raw_proxies: &[Mapping]) -> Allowance {
    if let Some(allowance) = allowance_from_headers(headers) {
        return allowance;
    }
    let notice = NoticeCluster::scan(raw_proxies);
    if notice.count >= 3
        && let Some(remaining) = notice.remaining
    {
        return Allowance::Remaining(AllowanceRemaining {
            remaining,
            source: "notice".to_owned(),
        });
    }
    Allowance::Unavailable
}

pub fn allowance_from_headers(headers: &[(String, String)]) -> Option<Allowance> {
    let canonical = headers
        .iter()
        .filter(|(name, _)| header_for_subscription_userinfo(name) == Some(true))
        .filter_map(|(_, value)| parse_userinfo(value))
        .next();
    if canonical.is_some() {
        return canonical;
    }
    headers
        .iter()
        .filter(|(name, _)| header_for_subscription_userinfo(name) == Some(false))
        .filter_map(|(_, value)| parse_userinfo(value))
        .next()
}

fn parse_userinfo(value: &str) -> Option<Allowance> {
    let mut fields = BTreeMap::new();
    for field in value.split(';') {
        let (key, value) = field.split_once('=')?;
        let key = key.trim().to_ascii_lowercase();
        if !matches!(key.as_str(), "upload" | "download" | "total" | "expire") {
            continue;
        }
        if fields.contains_key(&key) {
            return None;
        }
        fields.insert(key, value.trim());
    }
    let upload = fields.get("upload")?.parse::<u64>().ok()?;
    let download = fields.get("download")?.parse::<u64>().ok()?;
    let total = fields.get("total")?.parse::<u64>().ok()?;
    let used = upload.checked_add(download)?;
    if total == 0 {
        return None;
    }
    let expires_at = fields
        .get("expire")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0);
    let remaining = total.saturating_sub(used);
    Some(Allowance::Full(AllowanceFull {
        total,
        used: total - remaining,
        remaining,
        expires_at,
        source: "header".to_owned(),
    }))
}

#[derive(Debug, Default)]
struct NoticeCluster {
    count: usize,
    remaining: Option<u64>,
    excluded_indices: BTreeSet<usize>,
}

impl NoticeCluster {
    fn scan(raw_proxies: &[Mapping]) -> Self {
        let names: Vec<String> = raw_proxies.iter().map(proxy_name).collect();
        let fingerprints: Vec<Option<String>> = raw_proxies.iter().map(proxy_fingerprint).collect();
        let candidates: BTreeSet<usize> = names
            .iter()
            .enumerate()
            .filter(|(_, name)| is_notice_label(name))
            .map(|(index, _)| index)
            .collect();
        let mut normal_fingerprints = BTreeSet::new();
        for (index, fingerprint) in fingerprints.iter().enumerate() {
            if !candidates.contains(&index)
                && let Some(fingerprint) = fingerprint
            {
                normal_fingerprints.insert(fingerprint.clone());
            }
        }
        let mut result = Self {
            count: candidates.len(),
            ..Self::default()
        };
        if result.count >= 3 {
            result.remaining = extract_notice_remaining(&names);
            for index in candidates {
                if fingerprints[index]
                    .as_ref()
                    .is_some_and(|fingerprint| normal_fingerprints.contains(fingerprint))
                {
                    result.excluded_indices.insert(index);
                }
            }
        }
        result
    }

    fn should_exclude(&self, index: usize) -> bool {
        self.excluded_indices.contains(&index)
    }
}

fn proxy_name(mapping: &Mapping) -> String {
    mapping
        .get(Value::String("name".into()))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn proxy_fingerprint(mapping: &Mapping) -> Option<String> {
    let field = |key: &str| {
        mapping
            .get(Value::String(key.into()))
            .and_then(|value| match value {
                Value::String(value) => Some(value.clone()),
                Value::Number(value) => Some(value.to_string()),
                Value::Bool(value) => Some(value.to_string()),
                _ => None,
            })
            .unwrap_or_default()
    };
    let kind = field("type").to_ascii_lowercase();
    let server = field("server");
    let port = field("port");
    if server.is_empty() || port.is_empty() {
        return None;
    }
    let secret = match kind.as_str() {
        "trojan" => field("password"),
        "ss" => format!("{}:{}", field("cipher"), field("password")),
        "vmess" | "vless" => field("uuid"),
        _ => return None,
    };
    let transport = format!("{}:{}", field("network"), field("ws-opts"));
    let mut digest = Sha256::new();
    for value in [
        &kind,
        &server,
        &port,
        &field("sni"),
        &field("servername"),
        &secret,
        &transport,
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    Some(
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn is_notice_label(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return false;
    }
    const LABELS: [&str; 35] = [
        "账号信息",
        "账户信息",
        "登录账号",
        "登录账户",
        "登录信息",
        "套餐信息",
        "套餐详情",
        "到期时间",
        "重置时间",
        "流量重置",
        "流量信息",
        "剩余流量",
        "已用流量",
        "总流量",
        "邮箱",
        "官网",
        "网址",
        "客服",
        "联系",
        "公告",
        "通知",
        "telegram",
        "消息",
        "account",
        "account info",
        "login",
        "plan",
        "quota",
        "expires",
        "expiry",
        "remaining",
        "email",
        "website",
        "contact",
        "traffic reset",
    ];
    LABELS.iter().any(|label| anchored_label(&name, label))
}

fn anchored_label(name: &str, label: &str) -> bool {
    if name == label {
        return true;
    }
    if let Some(rest) = name.strip_prefix(label) {
        let rest = rest.trim_start();
        return rest.is_empty()
            || rest.starts_with([':', '：', '-', '|', '/', '·', '(', '（'])
            || rest.chars().next().is_some_and(|c| c.is_ascii_digit());
    }
    false
}

fn extract_notice_remaining(names: &[String]) -> Option<u64> {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    let regex = REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)流量重置[^\r\n]{0,80}?剩余\s*([0-9]+(?:\.[0-9]+)?)\s*(TB|GB|MB|T|G|M|B|字节)",
        )
        .expect("notice quota regex")
    });
    for name in names {
        if let Some(captures) = regex.captures(name) {
            let amount: f64 = captures.get(1)?.as_str().parse().ok()?;
            if !amount.is_finite() || amount < 0.0 {
                return None;
            }
            let multiplier = match captures.get(2)?.as_str().to_ascii_uppercase().as_str() {
                "TB" | "T" => 1024u64.pow(4),
                "GB" | "G" => 1024u64.pow(3),
                "MB" | "M" => 1024u64.pow(2),
                "B" | "字节" => 1,
                _ => return None,
            };
            let bytes = amount * multiplier as f64;
            if !bytes.is_finite() || bytes > u64::MAX as f64 {
                return None;
            }
            return Some(bytes.round() as u64);
        }
    }
    None
}

fn filter_and_sanitize_nodes(raw_proxies: Vec<Mapping>) -> Result<(Vec<ParsedNode>, usize, usize)> {
    let notice = NoticeCluster::scan(&raw_proxies);
    let mut nodes = Vec::new();
    let mut unsupported = 0usize;
    for (index, mapping) in raw_proxies.iter().enumerate() {
        if notice.should_exclude(index) {
            continue;
        }
        let Some(kind) = mapping
            .get(Value::String("type".into()))
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase)
        else {
            bail!("The subscription contains a malformed proxy entry.");
        };
        if !SUPPORTED_TYPES.contains(&kind.as_str()) {
            unsupported += 1;
            continue;
        }
        let node = sanitize_proxy(mapping, index).map_err(|_| {
            anyhow::anyhow!("The subscription contains an invalid supported proxy entry.")
        })?;
        nodes.push(node);
    }
    let mut identities = HashMap::<String, usize>::new();
    for node in &mut nodes {
        let count = identities.entry(node.source_id.clone()).or_default();
        *count += 1;
        if *count > 1 {
            node.source_id = format!("{}#{}", node.source_id, count);
        }
    }
    Ok((nodes, notice.excluded_indices.len(), unsupported))
}

fn sanitize_proxy(mapping: &Mapping, index: usize) -> std::result::Result<ParsedNode, ()> {
    let name = sanitize_node_name(&proxy_name(mapping), index);
    let kind = mapping
        .get(Value::String("type".into()))
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase)
        .ok_or(())?;
    if !SUPPORTED_TYPES.contains(&kind.as_str()) {
        return Err(());
    }
    let server = mapping
        .get(Value::String("server".into()))
        .and_then(Value::as_str)
        .filter(|value| {
            !value.trim().is_empty() && !value.chars().any(|c| c.is_control() || c == ',')
        })
        .ok_or(())?
        .trim()
        .to_owned();
    let port = mapping
        .get(Value::String("port".into()))
        .and_then(|value| match value {
            Value::Number(number) => number.as_u64(),
            Value::String(string) => string.parse::<u64>().ok(),
            _ => None,
        })
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or(())?;
    let network = mapping
        .get(Value::String("network".into()))
        .and_then(Value::as_str)
        .unwrap_or("tcp")
        .to_ascii_lowercase();
    if !matches!(network.as_str(), "tcp" | "ws" | "grpc") {
        return Err(());
    }

    let allowed: &[&str] = match kind.as_str() {
        "trojan" => &[
            "type",
            "server",
            "port",
            "password",
            "sni",
            "alpn",
            "network",
            "ws-opts",
            "grpc-opts",
            "skip-cert-verify",
            "client-fingerprint",
            "udp",
        ],
        "ss" => &[
            "type",
            "server",
            "port",
            "cipher",
            "password",
            "udp",
            "plugin",
            "plugin-opts",
        ],
        "vmess" => &[
            "type",
            "server",
            "port",
            "uuid",
            "alterId",
            "cipher",
            "udp",
            "tls",
            "servername",
            "network",
            "ws-opts",
            "grpc-opts",
            "skip-cert-verify",
            "alpn",
            "client-fingerprint",
            "packet-encoding",
        ],
        "vless" => &[
            "type",
            "server",
            "port",
            "uuid",
            "flow",
            "udp",
            "tls",
            "servername",
            "network",
            "ws-opts",
            "grpc-opts",
            "reality-opts",
            "skip-cert-verify",
            "alpn",
            "client-fingerprint",
            "packet-encoding",
        ],
        _ => &[],
    };
    let mut clean = Mapping::new();
    for key in allowed {
        if let Some(value) = mapping.get(Value::String((*key).to_owned())) {
            match *key {
                "ws-opts" => {
                    if let Some(value) = sanitize_ws_opts(value) {
                        clean.insert(Value::String((*key).to_owned()), value);
                    }
                }
                "grpc-opts" => {
                    if let Some(value) = sanitize_grpc_opts(value) {
                        clean.insert(Value::String((*key).to_owned()), value);
                    }
                }
                "reality-opts" => {
                    if let Some(value) = sanitize_reality_opts(value) {
                        clean.insert(Value::String((*key).to_owned()), value);
                    }
                }
                "plugin" => {
                    let plugin = value.as_str().unwrap_or_default();
                    if !matches!(plugin, "obfs" | "v2ray-plugin" | "shadow-tls") {
                        return Err(());
                    }
                    clean.insert(Value::String((*key).to_owned()), value.clone());
                }
                "plugin-opts" => {
                    if let Some(value) = sanitize_scalar_mapping(
                        value,
                        &["mode", "host", "path", "tls", "mux", "headers", "password"],
                    ) {
                        clean.insert(Value::String((*key).to_owned()), value);
                    }
                }
                "password" | "uuid" | "cipher" => {
                    let value = value.as_str().filter(|value| !value.is_empty()).ok_or(())?;
                    clean.insert(
                        Value::String((*key).to_owned()),
                        Value::String(value.to_owned()),
                    );
                }
                "server" | "port" => {}
                _ => {
                    clean.insert(Value::String((*key).to_owned()), value.clone());
                }
            }
        }
    }
    if !clean.contains_key(Value::String("type".into())) {
        return Err(());
    }
    let required = match kind.as_str() {
        "trojan" | "ss" => "password",
        "vmess" | "vless" => "uuid",
        _ => return Err(()),
    };
    if !clean.contains_key(Value::String(required.into())) {
        return Err(());
    }
    if let Some(network) = clean.get(Value::String("network".into()))
        && network
            .as_str()
            .is_some_and(|value| !matches!(value, "tcp" | "ws" | "grpc"))
    {
        return Err(());
    }
    clean.insert(
        Value::String("server".into()),
        Value::String(server.clone()),
    );
    clean.insert(Value::String("port".into()), Value::from(port));
    if !clean.contains_key(Value::String("udp".into())) {
        clean.insert(Value::String("udp".into()), Value::Bool(false));
    }
    let secret_material = proxy_fingerprint(mapping).unwrap_or_default();
    let protocol = match kind.as_str() {
        "trojan" => "Trojan",
        "ss" => "Shadowsocks",
        "vmess" => "VMess",
        "vless" => "VLESS",
        _ => "Unknown",
    }
    .to_owned();
    let source_id = source_identity(&kind, &name, &clean);
    Ok(ParsedNode {
        source_id,
        display_name: name,
        protocol,
        server,
        port,
        secret_material,
        proxy: Value::Mapping(clean),
    })
}

fn sanitize_node_name(raw: &str, index: usize) -> String {
    let mut name: String = raw
        .trim()
        .chars()
        .filter(|c| !c.is_control() && *c != ',' && *c != '\n' && *c != '\r')
        .take(80)
        .collect();
    name = name.trim().to_owned();
    if name.is_empty() {
        name = format!("Unnamed node {}", index + 1);
    }
    name
}

fn source_identity(kind: &str, name: &str, clean: &Mapping) -> String {
    let field = |key: &str| {
        clean
            .get(Value::String(key.into()))
            .map(|value| serde_json::to_string(value).unwrap_or_default())
            .unwrap_or_default()
    };
    let mut digest = Sha256::new();
    for part in [
        kind,
        name,
        &field("server"),
        &field("port"),
        &field("sni"),
        &field("servername"),
        &field("network"),
        &field("ws-opts"),
        &field("grpc-opts"),
    ] {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn stable_node_id(service_id: &str, source_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(service_id.as_bytes());
    digest.update([0]);
    digest.update(source_id.as_bytes());
    let digest = digest.finalize();
    format!(
        "node_{}",
        &digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()[..24]
    )
}

pub fn node_core_name(node_id: &str) -> String {
    let suffix = node_id.strip_prefix("node_").unwrap_or(node_id);
    format!("CF-{suffix}")
}

fn sanitize_ws_opts(value: &Value) -> Option<Value> {
    sanitize_scalar_mapping(
        value,
        &[
            "path",
            "max-early-data",
            "early-data-header-name",
            "headers",
        ],
    )
}

fn sanitize_grpc_opts(value: &Value) -> Option<Value> {
    sanitize_scalar_mapping(value, &["grpc-service-name"])
}

fn sanitize_reality_opts(value: &Value) -> Option<Value> {
    sanitize_scalar_mapping(value, &["public-key", "short-id"])
}

fn sanitize_scalar_mapping(value: &Value, allowed: &[&str]) -> Option<Value> {
    let mapping = value.as_mapping()?;
    let mut clean = Mapping::new();
    for key in allowed {
        let Some(value) = mapping.get(Value::String((*key).to_owned())) else {
            continue;
        };
        if *key == "headers" {
            let Some(headers) = value.as_mapping() else {
                continue;
            };
            let mut safe_headers = Mapping::new();
            if let Some(host) = headers.get(Value::String("Host".into())) {
                safe_headers.insert(Value::String("Host".into()), host.clone());
            }
            if !safe_headers.is_empty() {
                clean.insert(
                    Value::String("headers".into()),
                    Value::Mapping(safe_headers),
                );
            }
        } else {
            match value {
                Value::String(_) | Value::Bool(_) | Value::Number(_) => {
                    clean.insert(Value::String((*key).to_owned()), value.clone());
                }
                _ => {}
            }
        }
    }
    Some(Value::Mapping(clean))
}

pub fn prepend_nodes(
    state: &mut crate::models::AppState,
    secrets: &mut crate::models::SecretStore,
    service_id: &str,
    service_title: &str,
    nodes: Vec<ParsedNode>,
) {
    for node in nodes {
        let id = stable_node_id(service_id, &node.source_id);
        let core_name = node_core_name(&id);
        state.nodes.push(NodeView {
            id: id.clone(),
            service_id: service_id.to_owned(),
            service_title: service_title.to_owned(),
            display_name: node.display_name,
            protocol: node.protocol,
            missing: false,
            core_name,
        });
        secrets.nodes.insert(id, node.proxy);
    }
}

pub fn parse_share_link(input: &str) -> Result<ParsedNode> {
    let input = input.trim();
    let lower = input.to_ascii_lowercase();
    if lower.starts_with("trojan://") {
        parse_trojan(input)
    } else if lower.starts_with("ss://") {
        parse_ss(input)
    } else if lower.starts_with("vmess://") {
        parse_vmess(input)
    } else if lower.starts_with("vless://") {
        parse_vless(input)
    } else {
        bail!(
            "Unsupported share link. Clashfork accepts Trojan, Shadowsocks, VMess, and VLESS links."
        )
    }
}

fn parse_trojan(input: &str) -> Result<ParsedNode> {
    let url = Url::parse(input).context("Invalid Trojan share link.")?;
    let password = url
        .password()
        .filter(|value| !value.is_empty())
        .or_else(|| (!url.username().is_empty()).then_some(url.username()))
        .map(|value| {
            percent_encoding::percent_decode_str(value)
                .decode_utf8_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    if password.is_empty() {
        bail!("Trojan share link is missing its password.");
    }
    let server = host(&url)?;
    let port = url.port().unwrap_or(443);
    let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
    let name = link_name(&url, "Trojan node");
    let mut clean = base_link_mapping("trojan", &server, port);
    clean.insert("password".into(), Value::String(password.to_owned()));
    add_tls_options(&mut clean, &query, true)?;
    add_transport(&mut clean, &query)?;
    finish_link_node(clean, name)
}

fn parse_vless(input: &str) -> Result<ParsedNode> {
    let url = Url::parse(input).context("Invalid VLESS share link.")?;
    let uuid = url.username();
    if uuid.is_empty() {
        bail!("VLESS share link is missing its UUID.");
    }
    let server = host(&url)?;
    let port = url.port().unwrap_or(443);
    let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
    let security = query.get("security").map(String::as_str).unwrap_or("none");
    if !matches!(security, "none" | "tls" | "reality") {
        bail!("This VLESS link uses an unsupported security mode.");
    }
    let name = link_name(&url, "VLESS node");
    let mut clean = base_link_mapping("vless", &server, port);
    clean.insert("uuid".into(), Value::String(uuid.to_owned()));
    if security != "none" {
        clean.insert("tls".into(), Value::Bool(true));
        if let Some(sni) = query.get("sni").or_else(|| query.get("serverName")) {
            clean.insert("servername".into(), Value::String(sni.to_owned()));
        }
        if let Some(fp) = query.get("fp") {
            clean.insert("client-fingerprint".into(), Value::String(fp.to_owned()));
        }
        if query.get("allowInsecure").is_some_and(|value| value == "1") {
            clean.insert("skip-cert-verify".into(), Value::Bool(true));
        }
        if let Some(flow) = query.get("flow") {
            clean.insert("flow".into(), Value::String(flow.to_owned()));
        }
        if security == "reality" {
            let mut reality = Mapping::new();
            if let Some(public_key) = query.get("pbk") {
                reality.insert("public-key".into(), Value::String(public_key.to_owned()));
            }
            if let Some(short_id) = query.get("sid") {
                reality.insert("short-id".into(), Value::String(short_id.to_owned()));
            }
            if reality.is_empty() {
                bail!("This VLESS Reality link is missing its public key.");
            }
            clean.insert("reality-opts".into(), Value::Mapping(reality));
        }
    }
    add_transport(&mut clean, &query)?;
    finish_link_node(clean, name)
}

fn parse_vmess(input: &str) -> Result<ParsedNode> {
    let encoded = input
        .strip_prefix("vmess://")
        .or_else(|| input.strip_prefix("VMESS://"))
        .context("Invalid VMess share link.")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim().trim_end_matches('/'))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded.trim()))
        .context("The VMess link payload is not valid base64.")?;
    let json: serde_json::Value =
        serde_json::from_slice(&bytes).context("The VMess link payload is not valid JSON.")?;
    let field = |key: &str| {
        json.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
    };
    let uuid = field("id");
    let server = field("add");
    let port = field("port")
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .context("The VMess link has an invalid port.")?;
    if uuid.is_empty() || server.is_empty() {
        bail!("The VMess link is missing its UUID or server.");
    }
    if server.chars().any(|c| c.is_control() || c == ',') {
        bail!("The VMess link contains an unsafe server value.");
    }
    let name = {
        let raw = field("ps");
        if raw.trim().is_empty() {
            "VMess node".to_owned()
        } else {
            sanitize_node_name(raw, 0)
        }
    };
    let mut clean = base_link_mapping("vmess", server, port);
    clean.insert("uuid".into(), Value::String(uuid.to_owned()));
    if let Some(cipher) = json.get("scy").and_then(serde_json::Value::as_str)
        && !cipher.is_empty()
    {
        clean.insert("cipher".into(), Value::String(cipher.to_owned()));
    }
    if let Some(aid) = json.get("aid").and_then(|value| value.as_u64())
        && aid > 0
    {
        clean.insert("alterId".into(), Value::from(aid));
    }
    let tls = field("tls");
    if matches!(tls, "tls" | "reality") {
        clean.insert("tls".into(), Value::Bool(true));
        if let Some(sni) = json.get("sni").and_then(serde_json::Value::as_str)
            && !sni.is_empty()
        {
            clean.insert("servername".into(), Value::String(sni.to_owned()));
        }
    }
    let network = field("net");
    if !network.is_empty() {
        let mut query = HashMap::new();
        query.insert("type".into(), network.to_owned());
        if let Some(host) = json.get("host").and_then(serde_json::Value::as_str) {
            query.insert("host".into(), host.to_owned());
        }
        if let Some(path) = json.get("path").and_then(serde_json::Value::as_str) {
            query.insert("path".into(), path.to_owned());
        }
        add_transport(&mut clean, &query)?;
    }
    finish_link_node(clean, name)
}

fn parse_ss(input: &str) -> Result<ParsedNode> {
    let rest = input
        .strip_prefix("ss://")
        .or_else(|| input.strip_prefix("SS://"))
        .context("Invalid Shadowsocks share link.")?;
    let (without_fragment, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let name = sanitize_node_name(
        &percent_encoding::percent_decode_str(fragment).decode_utf8_lossy(),
        0,
    );
    let mut base64_part;
    let mut query = HashMap::new();
    let plain;
    if let Some(query_index) = without_fragment.find('?') {
        let (head, tail) = without_fragment.split_at(query_index);
        query = form_urlencoded::parse(tail[1..].as_bytes())
            .into_owned()
            .collect();
        plain = head.to_owned();
    } else {
        plain = without_fragment.to_owned();
    }
    let (userinfo, hostport) = if let Some(at) = plain.rfind('@') {
        (plain[..at].to_owned(), plain[at + 1..].to_owned())
    } else {
        base64_part = plain.clone();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(base64_part.trim_end_matches('/'))
            .or_else(|_| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(base64_part.trim_end_matches('/'))
            })
            .context("The Shadowsocks link payload is not valid base64.")?;
        base64_part =
            String::from_utf8(decoded).context("The Shadowsocks link payload is not UTF-8.")?;
        let at = base64_part
            .rfind('@')
            .context("The Shadowsocks link does not contain a server.")?;
        (
            base64_part[..at].to_owned(),
            base64_part[at + 1..].to_owned(),
        )
    };
    let decoded_userinfo = if userinfo.contains(':') {
        percent_encoding::percent_decode_str(&userinfo)
            .decode_utf8_lossy()
            .into_owned()
    } else {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(userinfo.trim_end_matches('/'))
            .or_else(|_| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(userinfo.trim_end_matches('/'))
            })
            .context("The Shadowsocks credentials are not valid base64.")?;
        String::from_utf8(decoded).context("The Shadowsocks credentials are not UTF-8.")?
    };
    let (cipher, password) = decoded_userinfo
        .split_once(':')
        .context("The Shadowsocks credentials are missing a cipher or password.")?;
    let (server, port_text) = hostport
        .rsplit_once(':')
        .context("The Shadowsocks link is missing its port.")?;
    if server.is_empty() {
        bail!("The Shadowsocks link is missing its server.");
    }
    let port = port_text
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .context("The Shadowsocks link has an invalid port.")?;
    let mut clean = base_link_mapping("ss", server, port);
    clean.insert("cipher".into(), Value::String(cipher.to_owned()));
    clean.insert("password".into(), Value::String(password.to_owned()));
    if let Some(plugin) = query.get("plugin") {
        apply_ss_plugin(&mut clean, plugin)?;
    }
    finish_link_node(clean, name)
}

fn apply_ss_plugin(clean: &mut Mapping, plugin: &str) -> Result<()> {
    let mut parts = plugin.split(';');
    let plugin_name = parts.next().unwrap_or_default();
    let mapped_name = match plugin_name {
        "obfs-local" | "simple-obfs" => "obfs",
        "v2ray-plugin" => "v2ray-plugin",
        _ => bail!("Unsupported Shadowsocks plugin."),
    };
    let mut opts = Mapping::new();
    for part in parts {
        let (key, value) = part.split_once('=').unwrap_or((part, "true"));
        match key {
            "obfs" | "mode" => {
                opts.insert("mode".into(), Value::String(value.to_owned()));
            }
            "obfs-host" | "host" => {
                opts.insert("host".into(), Value::String(value.to_owned()));
            }
            "path" => {
                opts.insert("path".into(), Value::String(value.to_owned()));
            }
            "tls" => {
                opts.insert("tls".into(), Value::Bool(true));
            }
            "mux" => {
                opts.insert("mux".into(), Value::Bool(true));
            }
            _ => bail!("Unsupported Shadowsocks plugin option."),
        }
    }
    clean.insert("plugin".into(), Value::String(mapped_name.to_owned()));
    if !opts.is_empty() {
        clean.insert("plugin-opts".into(), Value::Mapping(opts));
    }
    Ok(())
}

fn base_link_mapping(kind: &str, server: &str, port: u16) -> Mapping {
    let mut clean = Mapping::new();
    clean.insert("type".into(), Value::String(kind.to_owned()));
    clean.insert("server".into(), Value::String(server.to_owned()));
    clean.insert("port".into(), Value::from(port));
    clean.insert("udp".into(), Value::Bool(false));
    clean
}

fn add_tls_options(clean: &mut Mapping, query: &HashMap<String, String>, tls: bool) -> Result<()> {
    if tls {
        clean.insert("tls".into(), Value::Bool(true));
    }
    if let Some(sni) = query.get("sni").or_else(|| query.get("peer")) {
        clean.insert(
            if tls { "sni" } else { "servername" }.into(),
            Value::String(sni.to_owned()),
        );
    }
    if let Some(fp) = query.get("fp") {
        clean.insert("client-fingerprint".into(), Value::String(fp.to_owned()));
    }
    if query
        .get("allowInsecure")
        .or_else(|| query.get("skip-cert-verify"))
        .is_some_and(|value| matches!(value.as_str(), "1" | "true"))
    {
        clean.insert("skip-cert-verify".into(), Value::Bool(true));
    }
    if let Some(alpn) = query.get("alpn") {
        clean.insert(
            "alpn".into(),
            Value::Sequence(
                alpn.split(',')
                    .filter(|part| !part.trim().is_empty())
                    .map(|part| Value::String(part.trim().to_owned()))
                    .collect(),
            ),
        );
    }
    Ok(())
}

fn add_transport(clean: &mut Mapping, query: &HashMap<String, String>) -> Result<()> {
    let network = query.get("type").map(String::as_str).unwrap_or("tcp");
    match network {
        "tcp" | "" => {}
        "ws" => {
            clean.insert("network".into(), Value::String("ws".into()));
            let mut ws = Mapping::new();
            let path = query.get("path").map(String::as_str).unwrap_or("/");
            ws.insert("path".into(), Value::String(path.to_owned()));
            if let Some(host) = query.get("host") {
                let mut headers = Mapping::new();
                headers.insert("Host".into(), Value::String(host.to_owned()));
                ws.insert("headers".into(), Value::Mapping(headers));
            }
            clean.insert("ws-opts".into(), Value::Mapping(ws));
        }
        "grpc" => {
            clean.insert("network".into(), Value::String("grpc".into()));
            let mut grpc = Mapping::new();
            let service = query
                .get("serviceName")
                .or_else(|| query.get("service-name"))
                .or_else(|| query.get("path"))
                .map(String::as_str)
                .unwrap_or("");
            if !service.is_empty() {
                grpc.insert(
                    "grpc-service-name".into(),
                    Value::String(service.to_owned()),
                );
            }
            clean.insert("grpc-opts".into(), Value::Mapping(grpc));
        }
        _ => bail!("Unsupported transport. Clashfork supports TCP, WebSocket, and gRPC links."),
    }
    Ok(())
}

fn host(url: &Url) -> Result<String> {
    let host = url
        .host_str()
        .context("The share link is missing its server.")?;
    if host.chars().any(|c| c.is_control() || c == ',') {
        bail!("The share link contains an unsafe server value.");
    }
    Ok(host.to_owned())
}

fn link_name(url: &Url, fallback: &str) -> String {
    let raw = url
        .fragment()
        .map(|fragment| percent_encoding::percent_decode_str(fragment).decode_utf8_lossy())
        .unwrap_or_default();
    if raw.trim().is_empty() {
        fallback.to_owned()
    } else {
        sanitize_node_name(&raw, 0)
    }
}

fn finish_link_node(clean: Mapping, name: String) -> Result<ParsedNode> {
    let server = clean
        .get("server")
        .and_then(Value::as_str)
        .context("The share link is missing its server.")?
        .to_owned();
    let port = clean
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .context("The share link has an invalid port.")?;
    let kind = clean
        .get("type")
        .and_then(Value::as_str)
        .context("The share link is missing its protocol.")?
        .to_owned();
    let protocol = match kind.as_str() {
        "trojan" => "Trojan",
        "ss" => "Shadowsocks",
        "vmess" => "VMess",
        "vless" => "VLESS",
        _ => bail!("Unsupported share link protocol."),
    }
    .to_owned();
    let source_id = source_identity(&kind, &name, &clean);
    let secret_material = source_identity(&kind, &name, &clean);
    Ok(ParsedNode {
        source_id,
        display_name: name,
        protocol,
        server,
        port,
        secret_material,
        proxy: Value::Mapping(clean),
    })
}

// Keep private credential material intentionally unusable for display or serialization.
impl std::fmt::Debug for ParsedSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ParsedSource")
            .field("title", &self.title)
            .field("node_count", &self.nodes.len())
            .field("unsupported_count", &self.unsupported_count)
            .field("notice_count", &self.notice_count)
            .finish()
    }
}

pub fn format_allowance(allowance: &Allowance) -> String {
    match allowance {
        Allowance::Full(allowance) => format!(
            "{} remaining; {} total",
            human_bytes(allowance.remaining),
            human_bytes(allowance.total)
        ),
        Allowance::Remaining(allowance) => {
            format!(
                "{} remaining (provider reported)",
                human_bytes(allowance.remaining)
            )
        }
        Allowance::Unavailable => "Allowance unavailable".to_owned(),
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{}{}", bytes, UNITS[unit])
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AppState, ConditionKind, Destination, MatchGroup, Miscellaneous, Mode, NodeView, Selection,
        ServiceView, SourceType,
    };

    #[test]
    #[ignore = "makes one credentialed HTTPS request to the configured stale Falemon host"]
    fn configured_falemon_refresh_uses_the_production_reqwest_path_safely() {
        assert_eq!(
            std::env::var("CLASHFORK_LIVE_FALEMON_PROBE").as_deref(),
            Ok("1"),
            "set CLASHFORK_LIVE_FALEMON_PROBE=1 to opt in to one credentialed provider request"
        );
        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .unwrap_or_else(|| panic!("LOCALAPPDATA is not available for the opt-in probe"));
        let data_root = std::path::PathBuf::from(local_app_data).join("Clashfork");
        let paths = crate::paths::AppPaths {
            runtime_dir: data_root.join("runtime"),
            logs_dir: data_root.join("logs"),
            state_file: data_root.join("state.json"),
            secrets_file: data_root.join("secrets.dat"),
            history_file: data_root.join("history.sqlite3"),
            data_root,
        };
        let state = crate::store::load_state(&paths).unwrap_or_else(|_| {
            panic!("The local app state could not be read for the opt-in probe")
        });
        let matching = state
            .services
            .iter()
            .filter(|service| {
                service.title.to_ascii_lowercase().contains("falemon")
                    && service.source_type == SourceType::Subscription
                    && service.stale
                    && service.last_error.as_deref().is_some_and(|error| {
                        error.contains("not a supported Clash/Mihomo profile or node list")
                    })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            matching.len(),
            1,
            "The reported stale Falemon subscription was not uniquely identified"
        );
        let service = matching[0];
        let configured_host = service
            .source_host
            .as_deref()
            .filter(|host| !host.is_empty())
            .unwrap_or_else(|| panic!("The configured Falemon host is missing"));
        let secrets = crate::store::load_secrets(&paths)
            .unwrap_or_else(|_| panic!("The protected Falemon credential could not be read"));
        let configured_url = secrets
            .subscriptions
            .get(&service.id)
            .unwrap_or_else(|| panic!("The protected Falemon URL is missing"));
        let parsed_url = Url::parse(configured_url)
            .unwrap_or_else(|_| panic!("The protected Falemon URL is invalid"));
        assert!(
            parsed_url.scheme() == "https"
                && parsed_url
                    .host_str()
                    .is_some_and(|host| host.eq_ignore_ascii_case(configured_host)),
            "The protected Falemon URL does not match its recorded HTTPS host"
        );

        let client = subscription_http_client().unwrap_or_else(|_| {
            panic!("The production subscription HTTP client could not be built")
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|_| panic!("The local async runtime could not be built"));
        let parsed = runtime
            .block_on(fetch_source_on_host(
                &client,
                configured_url,
                configured_host,
            ))
            .unwrap_or_else(|error| {
                let text = error.to_string().to_ascii_lowercase();
                let category = if text.contains("timed out") || text.contains("timeout") {
                    "timeout"
                } else if text.contains("http ") {
                    "http-status"
                } else if text.contains("html/error page") {
                    "html-error-page"
                } else if text.contains("redirect") {
                    "redirect-or-host-policy"
                } else if text.contains("supported clash/mihomo") {
                    "unsupported-document-shape"
                } else if text.contains("no supported") {
                    "no-supported-nodes"
                } else {
                    "safe-fetch-or-parse-error"
                };
                panic!("The configured Falemon probe failed safely (category={category})")
            });
        let summary = parsed.fetch_summary.unwrap_or_else(|| {
            panic!("The configured Falemon probe did not use an HTTPS response")
        });
        eprintln!(
            "safe configured refresh: status={} redirects={} media_type={} encoding={} decoded_bytes={} parse_location={} supported_nodes={} unsupported_rows={} notice_rows={}",
            summary.response.status,
            summary.response.redirects,
            summary.response.media_type,
            summary.response.encoding,
            summary.response.decoded_body_bytes,
            summary.shape.label(),
            summary.supported_nodes,
            summary.unsupported_rows,
            summary.notice_rows,
        );
        assert!(
            summary.supported_nodes > 0,
            "The provider returned no supported nodes"
        );
    }
    use crate::models::{MatchCondition, Ruleset, RulesetEntry};
    use crate::rules::{ControllerConfig, build_runtime_yaml};

    fn synthetic_fixture() -> (Vec<Mapping>, Vec<(String, String)>) {
        let mut proxies = Vec::new();
        for index in 0..69 {
            let mut node = Mapping::new();
            node.insert(
                "name".into(),
                Value::String(format!("Singapore {index:02}")),
            );
            node.insert("type".into(), Value::String("trojan".into()));
            node.insert(
                "server".into(),
                Value::String(format!("s{index}.fixture.invalid")),
            );
            node.insert("port".into(), Value::from(443));
            node.insert(
                "password".into(),
                Value::String(format!("fixture-secret-{index}")),
            );
            node.insert("sni".into(), Value::String("fixture.invalid".into()));
            proxies.push(node);
        }
        // Notice aliases deliberately share the first real node's full connection tuple.
        for label in [
            "账号信息",
            "登录账户",
            "套餐信息",
            "流量重置 2026-11-01 剩余 64GB",
            "剩余流量 64GB",
            "Telegram",
            "邮箱",
            "官网",
            "到期时间",
            "消息: 5条未读，在APP查看",
        ] {
            let mut node = Mapping::new();
            node.insert("name".into(), Value::String(label.into()));
            node.insert("type".into(), Value::String("trojan".into()));
            node.insert("server".into(), Value::String("s0.fixture.invalid".into()));
            node.insert("port".into(), Value::from(443));
            node.insert("password".into(), Value::String("fixture-secret-0".into()));
            node.insert("sni".into(), Value::String("fixture.invalid".into()));
            proxies.push(node);
        }
        let headers = vec![(
            "subscription-userinfo".to_owned(),
            "upload=1073741824; download=2147483648; total=68719476736; expire=1780000000"
                .to_owned(),
        )];
        (proxies, headers)
    }

    fn app_state_with(node: &ParsedNode) -> (AppState, crate::models::SecretStore) {
        let id = stable_node_id("svc", &node.source_id);
        let mut state = AppState::default();
        state.services.push(ServiceView {
            id: "svc".into(),
            title: "Falemon".into(),
            source_type: SourceType::Subscription,
            source_host: Some("fixture.invalid".into()),
            node_count: 1,
            missing_count: 0,
            allowance: Allowance::Unavailable,
            stale: false,
            last_refresh_at: None,
            last_error: None,
        });
        state.nodes.push(NodeView {
            id: id.clone(),
            service_id: "svc".into(),
            service_title: "Falemon".into(),
            display_name: node.display_name.clone(),
            protocol: node.protocol.clone(),
            missing: false,
            core_name: node_core_name(&id),
        });
        state.match_groups.push(MatchGroup {
            id: "g1".into(),
            name: "OpenAI".into(),
            conditions: vec![
                MatchCondition {
                    id: "c1".into(),
                    kind: ConditionKind::ProcessName,
                    value: "ChatGPT.exe".into(),
                },
                MatchCondition {
                    id: "c2".into(),
                    kind: ConditionKind::ProcessName,
                    value: "codex.exe".into(),
                },
                MatchCondition {
                    id: "c3".into(),
                    kind: ConditionKind::DomainSuffix,
                    value: "chatgpt.com".into(),
                },
            ],
        });
        state.rulesets.push(Ruleset {
            id: "r1".into(),
            name: "AI".into(),
            entries: vec![RulesetEntry {
                id: "e1".into(),
                match_group_id: "g1".into(),
                destination: Destination::Node {
                    node_id: id.clone(),
                },
            }],
            miscellaneous: Miscellaneous::Direct,
        });
        state.selection = Selection {
            mode: Mode::Rule,
            ruleset_id: Some("r1".into()),
            global_node_id: None,
        };
        let mut secrets = crate::models::SecretStore::default();
        secrets.nodes.insert(id, node.proxy.clone());
        (state, secrets)
    }

    #[test]
    fn subscription_parser_accepts_profiles_proxy_lists_and_encoded_share_lists() {
        let yaml_profile = "\u{feff}proxies:\n  - name: Fixture\n    type: trojan\n    server: fixture.invalid\n    port: 443\n    password: fixture-password\n";
        let SubscriptionDocument::Profile(profile) =
            parse_subscription_document(yaml_profile).unwrap()
        else {
            panic!("YAML profile should remain a profile");
        };
        assert_eq!(
            mapping_sequence(profile.get("proxies").unwrap(), "bad proxies")
                .unwrap()
                .len(),
            1
        );

        let bare_list = "- name: Fixture\n  type: trojan\n  server: fixture.invalid\n  port: 443\n  password: fixture-password\n";
        let SubscriptionDocument::Proxies(proxies) =
            parse_subscription_document(bare_list).unwrap()
        else {
            panic!("a bare YAML node list should be accepted");
        };
        assert_eq!(filter_and_sanitize_nodes(proxies).unwrap().0.len(), 1);

        let share_list = "\u{feff}  trojan://fixture-password@fixture.invalid:443#Fixture\n消息: 5条未读，在APP查看\n";
        let SubscriptionDocument::Proxies(proxies) =
            parse_subscription_document(share_list).unwrap()
        else {
            panic!("a plain share-link list should be accepted");
        };
        assert_eq!(filter_and_sanitize_nodes(proxies).unwrap().0.len(), 1);

        let encoded = STANDARD.encode("trojan://fixture-password@fixture.invalid:443#Fixture\n");
        let SubscriptionDocument::Proxies(proxies) = parse_subscription_document(&encoded).unwrap()
        else {
            panic!("a base64-wrapped share-link list should be accepted");
        };
        assert_eq!(filter_and_sanitize_nodes(proxies).unwrap().0.len(), 1);
    }

    #[test]
    fn subscription_parser_rejects_error_pages_and_partial_proxy_lists() {
        assert!(parse_subscription_document("<html>access denied</html>").is_err());
        assert!(parse_subscription_document(" ﻿ \n ").is_err());

        let malformed_list = "proxies:\n  - name: Good\n    type: trojan\n    server: good.invalid\n    port: 443\n    password: ok\n  - malformed entry\n";
        let SubscriptionDocument::Profile(profile) =
            parse_subscription_document(malformed_list).unwrap()
        else {
            panic!("root profile should parse before its proxy rows are validated");
        };
        assert!(mapping_sequence(profile.get("proxies").unwrap(), "bad proxies").is_err());

        let partially_invalid_supported_node = "- name: Good\n  type: trojan\n  server: good.invalid\n  port: 443\n  password: ok\n- name: Broken\n  type: trojan\n  server: broken.invalid\n  port: 443\n";
        let SubscriptionDocument::Proxies(proxies) =
            parse_subscription_document(partially_invalid_supported_node).unwrap()
        else {
            panic!("bare YAML proxy list should parse");
        };
        assert!(filter_and_sanitize_nodes(proxies).is_err());
    }

    #[test]
    fn body_shape_overrides_a_misleading_html_media_type_but_rejects_html_bodies() {
        let profile = "\u{feff}proxies:\n  - name: Fixture\n    type: trojan\n    server: fixture.invalid\n    port: 443\n    password: fixture-password\n";
        let (document, shape) = parse_response_document(profile, "text/html").unwrap();
        assert!(matches!(document, SubscriptionDocument::Profile(_)));
        assert_eq!(shape, DocumentShape::ProfileMapping);

        let error = parse_response_document(
            "<!doctype html><html><body>login</body></html>",
            "application/yaml",
        )
        .unwrap_err();
        assert!(error.to_string().contains("HTML/error page"));
        assert!(parse_subscription_document("403 Forbidden").is_err());
        assert!(safe_media_type("text/html; charset=utf-8") == "html");
        assert!(safe_content_encoding("GZip") == "gzip");
    }

    #[test]
    fn subscription_negotiation_uses_clash_verge_ua_and_text_yaml_media_types() {
        let client = subscription_http_client().unwrap();
        let url = Url::parse("https://fixture.invalid/subscription").unwrap();
        let request = subscription_request(&client, &url).build().unwrap();
        let headers = request.headers();
        assert_eq!(
            headers
                .get(reqwest::header::USER_AGENT)
                .unwrap()
                .to_str()
                .unwrap(),
            format!("clash-verge/v{}", env!("CARGO_PKG_VERSION"))
        );
        let accept = headers
            .get(reqwest::header::ACCEPT)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(accept.contains("application/yaml"));
        assert!(accept.contains("text/plain"));
    }

    #[test]
    fn mihomo_proxy_provider_headers_are_applied_and_kept_sensitive() {
        let provider: Mapping = serde_yaml_ng::from_str(
            "type: http\nheader:\n  Authorization:\n    - Bearer fixture-token\n  User-Agent:\n    - mihomo/1.18.3\n",
        )
        .unwrap();
        let custom = parse_provider_headers(&provider).unwrap();
        let client = reqwest::Client::new();
        let url = Url::parse("https://fixture.invalid/provider").unwrap();
        let request = subscription_request(&client, &url)
            .headers(custom)
            .build()
            .unwrap();
        let headers = request.headers();
        assert_eq!(
            headers
                .get(reqwest::header::AUTHORIZATION)
                .unwrap()
                .to_str()
                .unwrap(),
            "Bearer fixture-token"
        );
        assert!(
            headers
                .get(reqwest::header::AUTHORIZATION)
                .unwrap()
                .is_sensitive()
        );
        assert_eq!(
            headers
                .get(reqwest::header::USER_AGENT)
                .unwrap()
                .to_str()
                .unwrap(),
            "mihomo/1.18.3"
        );
    }

    #[test]
    fn provider_credentials_are_not_forwarded_across_redirect_origins() {
        let original = Url::parse("https://provider.invalid/list").unwrap();
        let same_host = Url::parse("https://provider.invalid/next").unwrap();
        let new_host = Url::parse("https://cdn.invalid/list").unwrap();
        let new_port = Url::parse("https://provider.invalid:8443/list").unwrap();
        assert!(same_origin(&original, &same_host));
        assert!(!same_origin(&original, &new_host));
        assert!(!same_origin(&original, &new_port));
        assert_eq!(
            resolve_https_redirect(&original, "/next").unwrap(),
            same_host
        );
        let downgrade =
            resolve_https_redirect(&original, "http://provider.invalid/list").unwrap_err();
        assert_eq!(
            downgrade.to_string(),
            "The subscription redirected outside HTTPS."
        );
    }

    #[test]
    fn invalid_provider_headers_are_rejected_without_echoing_values() {
        let provider: Mapping =
            serde_yaml_ng::from_str("type: http\nheader:\n  Authorization: [fixture-secret, 12]\n")
                .unwrap();
        let error = parse_provider_headers(&provider).unwrap_err();
        assert_eq!(error.to_string(), "A node provider's headers are invalid.");
        assert!(!format!("{error:#}").contains("fixture-secret"));
    }

    #[test]
    fn synthetic_falemon_shape_filters_notice_aliases_and_uses_header() {
        let (proxies, headers) = synthetic_fixture();
        let allowance = choose_allowance(&headers, &proxies);
        let (nodes, notice_count, unsupported) = filter_and_sanitize_nodes(proxies).unwrap();
        assert_eq!(nodes.len(), 69);
        assert_eq!(notice_count, 10);
        assert_eq!(unsupported, 0);
        assert!(nodes.iter().all(|node| {
            node.display_name != "邮箱"
                && node.display_name != "账号信息"
                && node.display_name != "消息: 5条未读，在APP查看"
        }));
        match &allowance {
            Allowance::Full(full) => {
                assert_eq!(full.total, 68719476736);
                assert_eq!(full.used, 3221225472);
                assert_eq!(full.remaining, 65498251264);
                assert_eq!(full.expires_at, Some(1780000000));
            }
            other => panic!("unexpected allowance: {other:?}"),
        }
        let preview = ParsedSource {
            title: "Falemon".into(),
            source_host: Some("fixture.invalid".into()),
            nodes,
            unsupported_count: 0,
            notice_count,
            allowance,
            fetch_summary: None,
        }
        .preview("preview".into(), SourceType::Subscription);
        let serialized = serde_json::to_string(&preview).unwrap();
        assert!(
            serialized.contains("fixture.invalid"),
            "the provider domain is allowed service metadata"
        );
        assert!(!serialized.contains("fixture-secret"));
        assert!(!serialized.contains("s0.fixture.invalid"));
        assert!(!serialized.contains("账号"));
    }

    #[test]
    fn valid_canonical_header_wins_over_prefixed_and_notice_fallback() {
        let mut headers = vec![
            (
                "x-subscription-userinfo".into(),
                "upload=1; download=1; total=100".into(),
            ),
            (
                "SUBSCRIPTION-USERINFO".into(),
                "upload=10; download=20; total=1000; expire=1800000000".into(),
            ),
        ];
        let proxies = synthetic_fixture().0;
        match choose_allowance(&headers, &proxies) {
            Allowance::Full(full) => {
                assert_eq!(full.total, 1000);
                assert_eq!(full.remaining, 970);
            }
            other => panic!("unexpected allowance: {other:?}"),
        }
        headers.remove(1);
        match choose_allowance(&headers, &proxies) {
            Allowance::Full(full) => {
                assert_eq!(full.total, 100);
                assert_eq!(full.remaining, 98);
            }
            other => panic!("unexpected allowance: {other:?}"),
        }
    }

    #[test]
    fn invalid_or_partial_header_uses_provider_notice_remaining_only() {
        let proxies = synthetic_fixture().0;
        let headers = vec![(
            "subscription-userinfo".into(),
            "upload=broken; total=100".into(),
        )];
        match choose_allowance(&headers, &proxies) {
            Allowance::Remaining(remaining) => {
                assert_eq!(remaining.remaining, 64 * 1024u64.pow(3));
                assert_eq!(remaining.source, "notice");
            }
            other => panic!("unexpected allowance: {other:?}"),
        }
        assert!(matches!(
            choose_allowance(&headers, &proxies[..69]),
            Allowance::Unavailable
        ));
        assert!(matches!(
            choose_allowance(&[], &proxies),
            Allowance::Remaining(_)
        ));
    }

    #[test]
    fn absence_of_both_sources_is_unavailable() {
        assert!(matches!(
            choose_allowance(&[], &synthetic_fixture().0[..69]),
            Allowance::Unavailable
        ));
    }

    #[test]
    fn notice_filter_requires_anchor_and_shared_tuple() {
        let mut proxies = synthetic_fixture().0[..69].to_vec();
        let mut real = proxies[0].clone();
        real.insert("name".into(), Value::String("TELEGRAM node".into()));
        proxies.push(real);
        let (nodes, _, _) = filter_and_sanitize_nodes(proxies).unwrap();
        assert!(
            nodes
                .iter()
                .any(|node| node.display_name == "TELEGRAM node"),
            "a similarly named real node must survive when there is no notice cluster"
        );
    }

    #[test]
    fn share_links_parse_without_leaking_credentials_into_names() {
        let node = parse_share_link(
            "trojan://fixture-password@example.invalid:443?security=tls&type=ws&path=%2Fws&sni=example.invalid#Singapore%20I0",
        )
        .unwrap();
        assert_eq!(node.display_name, "Singapore I0");
        assert_eq!(node.protocol, "Trojan");
        let node = parse_share_link("vless://123e4567-e89b-12d3-a456-426614174000@example.invalid:443?type=grpc&security=tls&serviceName=grpc#Node").unwrap();
        assert_eq!(node.protocol, "VLESS");
        let node = parse_share_link(
            "ss://YWVzLTI1Ni1nY206Zml4dHVyZS1wYXNzd29yZA==@example.invalid:8388#SS%20node",
        )
        .unwrap();
        assert_eq!(node.protocol, "Shadowsocks");
        assert!(parse_share_link("http://example.invalid").is_err());
        assert!(parse_share_link("hysteria2://secret@example.invalid:443").is_err());
    }

    #[test]
    fn synthetic_256kb_fixture_has_69_nodes_and_nine_filtered_notices() {
        let body = include_str!("../tests/fixtures/falemon_style_78.yaml");
        assert!(body.len() > 256 * 1024);
        let value: Value = serde_yaml_ng::from_str(body).expect("synthetic fixture parses");
        let root = value.as_mapping().expect("fixture root mapping");
        let proxies: Vec<Mapping> = root
            .get("proxies")
            .and_then(Value::as_sequence)
            .expect("fixture proxies")
            .iter()
            .filter_map(Value::as_mapping)
            .cloned()
            .collect();
        assert_eq!(proxies.len(), 78);
        let headers = vec![
            (
                "subscription-userinfo".to_owned(),
                "upload=8053063680; download=9663676416; total=107374182400; expire=1790000000"
                    .to_owned(),
            ),
            (
                "x-subscription-userinfo".to_owned(),
                "upload=0; download=0; total=1".to_owned(),
            ),
        ];
        let allowance = choose_allowance(&headers, &proxies);
        let (nodes, notice_count, unsupported) = filter_and_sanitize_nodes(proxies).unwrap();
        assert_eq!(nodes.len(), 69);
        assert_eq!(notice_count, 9);
        assert_eq!(unsupported, 0);
        match allowance {
            Allowance::Full(full) => assert_eq!(full.total, 107374182400),
            other => panic!("unexpected allowance: {other:?}"),
        }
        let serialized = serde_json::to_string(
            &nodes
                .iter()
                .map(|node| &node.display_name)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(!serialized.contains("fixture-credential"));
        assert!(!serialized.contains("sg0.fixture.invalid"));
        assert!(!serialized.contains("邮箱"));
        assert!(!serialized.contains("账号信息"));
    }

    #[test]
    fn vmess_share_link_and_safe_title_fallbacks_are_supported() {
        let link = "vmess://eyJ2IjoiMiIsInBzIjoiVk1lc3MgVGVzdCIsImFkZCI6ImV4YW1wbGUuaW52YWxpZCIsInBvcnQiOiI0NDMiLCJpZCI6IjEyM2U0NTY3LWU4OWItMTJkMy1hNDU2LTQyNjYxNDE3NDAwMCIsImFpZCI6IjAiLCJuZXQiOiJ3cyIsInR5cGUiOiJub25lIiwiaG9zdCI6ImV4YW1wbGUuaW52YWxpZCIsInBhdGgiOiIvd3MiLCJ0bHMiOiJ0bHMifQ==";
        let node = parse_share_link(link).unwrap();
        assert_eq!(node.protocol, "VMess");
        assert_eq!(node.display_name, "VMess Test");
        assert_eq!(
            source_identity(
                "vendor.io",
                &node.display_name,
                node.proxy.as_mapping().unwrap()
            )
            .len(),
            64
        );
        assert_eq!(
            service_title(
                &[("profile-title".into(), "Falemon".into())],
                Some("files.example.invalid")
            ),
            "Falemon"
        );
        assert_eq!(
            service_title(
                &[(
                    "content-disposition".into(),
                    "attachment; filename=\"safe-name.yaml\"".into()
                )],
                Some("files.example.invalid")
            ),
            "safe-name"
        );
        assert_eq!(
            service_title(&[], Some("files.example.invalid")),
            "files.example.invalid"
        );
        assert_eq!(service_title(&[], None), "Subscription");
    }

    #[test]
    fn imported_nodes_from_equal_names_in_two_services_stay_distinct() {
        let (proxies, _) = synthetic_fixture();
        let (mut nodes, _, _) = filter_and_sanitize_nodes(proxies[..1].to_vec()).unwrap();
        let node = nodes.remove(0);
        let id_a = stable_node_id("service-a", &node.source_id);
        let id_b = stable_node_id("service-b", &node.source_id);
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn build_rule_config_has_immediate_udp_reject_guard() {
        let (proxies, _) = synthetic_fixture();
        let (mut raw_nodes, _, _) = filter_and_sanitize_nodes(proxies[..1].to_vec()).unwrap();
        let node = raw_nodes.remove(0);
        let (state, secrets) = app_state_with(&node);
        let selection = state.selection.clone();
        let compiled = build_runtime_yaml(
            &state,
            &secrets,
            &selection,
            &ControllerConfig {
                port: 19197,
                secret: "fixture-secret".into(),
                dns_port: 19198,
            },
            true,
        )
        .unwrap();
        let value: Value = serde_yaml_ng::from_str(&compiled.yaml).unwrap();
        let rules: Vec<String> = value["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            rules[0],
            format!("PROCESS-NAME,ChatGPT.exe,{}", state.nodes[0].core_name)
        );
        assert_eq!(rules[1], "PROCESS-NAME,ChatGPT.exe,REJECT");
        assert_eq!(
            rules[4],
            "DOMAIN-SUFFIX,chatgpt.com,".to_owned() + &state.nodes[0].core_name
        );
        assert_eq!(rules[5], "DOMAIN-SUFFIX,chatgpt.com,REJECT");
        assert_eq!(rules.last().unwrap(), "MATCH,DIRECT");
    }
}
