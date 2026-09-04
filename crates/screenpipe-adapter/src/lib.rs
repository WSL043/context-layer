use std::{collections::BTreeMap, io::Read, net::IpAddr, time::Duration as StdDuration};

use reqwest::{
    StatusCode,
    blocking::{Client, Response},
};
use serde::Deserialize;
use thiserror::Error;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
use url::{Host, Url};

const SEARCH_PAGE_SIZE: usize = 20;
const MAX_RESULTS_PER_SOURCE: usize = 20_000;
const INITIAL_BACKFILL: Duration = Duration::minutes(5);
const CURSOR_OVERLAP: Duration = Duration::seconds(2);
const MAX_SEARCH_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SCREENSHOT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenTextSource {
    Accessibility,
    Ocr,
}

impl ScreenTextSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accessibility => "accessibility",
            Self::Ocr => "ocr",
        }
    }

    fn priority(self) -> u8 {
        match self {
            Self::Accessibility => 2,
            Self::Ocr => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenpipeCursor {
    pub frame_id: u64,
    pub captured_at: OffsetDateTime,
}

impl ScreenpipeCursor {
    pub fn from_frame(frame: &ScreenpipeFrame) -> Self {
        Self {
            frame_id: frame.frame_id,
            captured_at: frame.captured_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenpipeFrame {
    pub frame_id: u64,
    pub captured_at: OffsetDateTime,
    pub app_name: Option<String>,
    pub window_name: Option<String>,
    pub text: String,
    pub text_source: ScreenTextSource,
    pub focused: Option<bool>,
}

pub struct ScreenpipeClient {
    base_url: Url,
    api_key: String,
    client: Client,
}

impl ScreenpipeClient {
    pub fn new(base_url: &str, api_key: impl Into<String>) -> Result<Self, ScreenpipeError> {
        let base_url = validate_loopback_base_url(base_url)?;
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(ScreenpipeError::MissingApiKey);
        }
        let client = Client::builder()
            .connect_timeout(StdDuration::from_secs(1))
            .timeout(StdDuration::from_secs(8))
            .build()?;
        Ok(Self {
            base_url,
            api_key,
            client,
        })
    }

    pub fn fetch_frames_since(
        &self,
        cursor: Option<&ScreenpipeCursor>,
        end_time: OffsetDateTime,
    ) -> Result<Vec<ScreenpipeFrame>, ScreenpipeError> {
        let start_time = match cursor {
            Some(cursor) => cursor
                .captured_at
                .checked_sub(CURSOR_OVERLAP)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH),
            None => end_time
                .checked_sub(INITIAL_BACKFILL)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH),
        };

        let mut candidates = Vec::new();
        candidates.extend(self.search_text_source(
            ScreenTextSource::Accessibility,
            start_time,
            end_time,
        )?);
        candidates.extend(self.search_text_source(ScreenTextSource::Ocr, start_time, end_time)?);

        if let Some(cursor) = cursor {
            let has_newer_timestamp = candidates.iter().any(|candidate| {
                candidate.captured_at > cursor.captured_at && candidate.frame_id <= cursor.frame_id
            });
            let has_newer_id = candidates
                .iter()
                .any(|candidate| candidate.frame_id > cursor.frame_id);
            if has_newer_timestamp && !has_newer_id {
                return Err(ScreenpipeError::SourceCursorReset {
                    last_frame_id: cursor.frame_id,
                });
            }
        }

        let mut by_frame = BTreeMap::<u64, ScreenpipeFrame>::new();
        for candidate in candidates {
            if cursor.is_some_and(|cursor| candidate.frame_id <= cursor.frame_id) {
                continue;
            }
            match by_frame.entry(candidate.frame_id) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(candidate);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    merge_candidate(entry.get_mut(), candidate);
                }
            }
        }

        Ok(by_frame.into_values().collect())
    }

    pub fn fetch_frame_png(&self, frame_id: u64) -> Result<Option<Vec<u8>>, ScreenpipeError> {
        let url = self.endpoint(&format!("frames/{frame_id}"))?;
        let response = self
            .client
            .get(url)
            .header("X-Screenpipe-Client", "api")
            .send()?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = require_success(response, "frames")?;
        Ok(Some(read_bounded(
            response,
            "frames",
            MAX_SCREENSHOT_BYTES,
        )?))
    }

    fn search_text_source(
        &self,
        source: ScreenTextSource,
        start_time: OffsetDateTime,
        end_time: OffsetDateTime,
    ) -> Result<Vec<ScreenpipeFrame>, ScreenpipeError> {
        let start = start_time.format(&Rfc3339)?;
        let end = end_time.format(&Rfc3339)?;
        let mut offset = 0usize;
        let mut collected = Vec::new();

        loop {
            if collected.len() >= MAX_RESULTS_PER_SOURCE {
                return Err(ScreenpipeError::TooManyResults {
                    content_type: source.as_str(),
                    limit: MAX_RESULTS_PER_SOURCE,
                });
            }

            let mut url = self.endpoint("search")?;
            url.query_pairs_mut()
                .append_pair("content_type", source.as_str())
                .append_pair("limit", &SEARCH_PAGE_SIZE.to_string())
                .append_pair("offset", &offset.to_string())
                .append_pair("start_time", &start)
                .append_pair("end_time", &end)
                .append_pair(
                    "fields",
                    "type,content.frame_id,content.timestamp,content.app_name,content.window_name,content.text,content.focused",
                );

            let response = self
                .client
                .get(url)
                .bearer_auth(&self.api_key)
                .header("X-Screenpipe-Client", "api")
                .send()?;
            if response.status() == StatusCode::FORBIDDEN
                || response.status() == StatusCode::UNAUTHORIZED
            {
                return Err(ScreenpipeError::Authentication);
            }
            let response = require_success(response, "search")?;
            let body = read_bounded(response, "search", MAX_SEARCH_RESPONSE_BYTES)?;
            let page: SearchResponse = serde_json::from_slice(&body)?;
            let page_len = page.data.len();
            for item in page.data {
                if let Some(frame) = item.into_frame(source)? {
                    collected.push(frame);
                }
            }
            offset = offset.saturating_add(page_len);

            if page_len < SEARCH_PAGE_SIZE
                || page
                    .pagination
                    .as_ref()
                    .is_some_and(|pagination| offset >= pagination.total)
            {
                break;
            }
        }

        Ok(collected)
    }

    fn endpoint(&self, path: &str) -> Result<Url, ScreenpipeError> {
        self.base_url
            .join(path)
            .map_err(ScreenpipeError::InvalidUrl)
    }
}

fn merge_candidate(existing: &mut ScreenpipeFrame, candidate: ScreenpipeFrame) {
    let existing_has_text = !existing.text.trim().is_empty();
    let candidate_has_text = !candidate.text.trim().is_empty();
    if (!existing_has_text && candidate_has_text)
        || (candidate_has_text
            && candidate.text_source.priority() > existing.text_source.priority())
    {
        existing.text = candidate.text;
        existing.text_source = candidate.text_source;
    }
    if existing.app_name.as_deref().is_none_or(str::is_empty) {
        existing.app_name = candidate.app_name;
    }
    if existing.window_name.as_deref().is_none_or(str::is_empty) {
        existing.window_name = candidate.window_name;
    }
    if existing.focused.is_none() {
        existing.focused = candidate.focused;
    }
    if candidate.captured_at > existing.captured_at {
        existing.captured_at = candidate.captured_at;
    }
}

fn validate_loopback_base_url(value: &str) -> Result<Url, ScreenpipeError> {
    let mut url = Url::parse(value).map_err(ScreenpipeError::InvalidUrl)?;
    if url.scheme() != "http" {
        return Err(ScreenpipeError::NonLoopbackBaseUrl(value.into()));
    }
    let loopback = match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        None => false,
    };
    if !loopback
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ScreenpipeError::NonLoopbackBaseUrl(value.into()));
    }
    if url.path().is_empty() {
        url.set_path("/");
    }
    if url.path() != "/" {
        return Err(ScreenpipeError::NonLoopbackBaseUrl(value.into()));
    }
    Ok(url)
}

fn require_success(
    response: Response,
    endpoint: &'static str,
) -> Result<Response, ScreenpipeError> {
    if response.status().is_success() {
        return Ok(response);
    }
    Err(ScreenpipeError::ApiStatus {
        endpoint,
        status: response.status().as_u16(),
    })
}

fn read_bounded(
    response: Response,
    endpoint: &'static str,
    limit: usize,
) -> Result<Vec<u8>, ScreenpipeError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(ScreenpipeError::ResponseTooLarge { endpoint, limit });
    }
    let mut bytes = Vec::new();
    response
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(ScreenpipeError::ResponseTooLarge { endpoint, limit });
    }
    Ok(bytes)
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<SearchItem>,
    pagination: Option<SearchPagination>,
}

#[derive(Deserialize)]
struct SearchPagination {
    total: usize,
}

#[derive(Deserialize)]
struct SearchItem {
    content: SearchContent,
}

impl SearchItem {
    fn into_frame(
        self,
        source: ScreenTextSource,
    ) -> Result<Option<ScreenpipeFrame>, ScreenpipeError> {
        let Some(frame_id) = value_to_u64(self.content.frame_id.as_ref()) else {
            return Ok(None);
        };
        let Some(timestamp) = self.content.timestamp.as_deref() else {
            return Ok(None);
        };
        let captured_at = OffsetDateTime::parse(timestamp, &Rfc3339)?;
        Ok(Some(ScreenpipeFrame {
            frame_id,
            captured_at,
            app_name: clean_optional(self.content.app_name),
            window_name: clean_optional(self.content.window_name),
            text: self.content.text.unwrap_or_default(),
            text_source: source,
            focused: self.content.focused,
        }))
    }
}

#[derive(Deserialize)]
struct SearchContent {
    frame_id: Option<serde_json::Value>,
    timestamp: Option<String>,
    app_name: Option<String>,
    window_name: Option<String>,
    text: Option<String>,
    focused: Option<bool>,
}

fn value_to_u64(value: Option<&serde_json::Value>) -> Option<u64> {
    match value? {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

#[derive(Debug, Error)]
pub enum ScreenpipeError {
    #[error("screenpipe API key is required")]
    MissingApiKey,
    #[error("screenpipe base URL must be plain HTTP on localhost/loopback only: {0}")]
    NonLoopbackBaseUrl(String),
    #[error("invalid URL: {0}")]
    InvalidUrl(#[source] url::ParseError),
    #[error("screenpipe authentication failed")]
    Authentication,
    #[error("screenpipe {endpoint} endpoint returned HTTP {status}")]
    ApiStatus { endpoint: &'static str, status: u16 },
    #[error("screenpipe {endpoint} response exceeded {limit} bytes")]
    ResponseTooLarge {
        endpoint: &'static str,
        limit: usize,
    },
    #[error("screenpipe {content_type} search exceeded the bounded {limit}-result poll")]
    TooManyResults {
        content_type: &'static str,
        limit: usize,
    },
    #[error("screenpipe frame IDs appear to have reset below persisted frame {last_frame_id}")]
    SourceCursorReset { last_frame_id: u64 },
    #[error("screenpipe HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("screenpipe response read failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("screenpipe JSON response was invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("screenpipe timestamp formatting failed: {0}")]
    TimestampFormat(#[from] time::error::Format),
    #[error("screenpipe timestamp parsing failed: {0}")]
    TimestampParse(#[from] time::error::Parse),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: u64, source: ScreenTextSource, text: &str) -> ScreenpipeFrame {
        ScreenpipeFrame {
            frame_id: id,
            captured_at: OffsetDateTime::from_unix_timestamp(1_700_000_000 + id as i64).unwrap(),
            app_name: Some("Example".into()),
            window_name: Some("Window".into()),
            text: text.into(),
            text_source: source,
            focused: Some(true),
        }
    }

    #[test]
    fn base_url_must_stay_on_loopback() {
        assert!(ScreenpipeClient::new("http://localhost:3030", "key").is_ok());
        assert!(ScreenpipeClient::new("http://127.0.0.1:3030/", "key").is_ok());
        assert!(ScreenpipeClient::new("http://[::1]:3030", "key").is_ok());
        assert!(ScreenpipeClient::new("https://localhost:3030", "key").is_err());
        assert!(ScreenpipeClient::new("http://example.com:3030", "key").is_err());
        assert!(ScreenpipeClient::new("http://localhost:3030/search", "key").is_err());
    }

    #[test]
    fn accessibility_text_wins_over_ocr_for_the_same_frame() {
        let mut existing = frame(7, ScreenTextSource::Ocr, "ocr fallback");
        merge_candidate(
            &mut existing,
            frame(7, ScreenTextSource::Accessibility, "accessibility text"),
        );
        assert_eq!(existing.text, "accessibility text");
        assert_eq!(existing.text_source, ScreenTextSource::Accessibility);
    }

    #[test]
    fn nonempty_ocr_can_fill_an_empty_accessibility_result() {
        let mut existing = frame(7, ScreenTextSource::Accessibility, "");
        merge_candidate(
            &mut existing,
            frame(7, ScreenTextSource::Ocr, "visible text"),
        );
        assert_eq!(existing.text, "visible text");
        assert_eq!(existing.text_source, ScreenTextSource::Ocr);
    }

    #[test]
    fn frame_id_parser_accepts_numeric_and_string_api_shapes() {
        assert_eq!(value_to_u64(Some(&serde_json::json!(42))), Some(42));
        assert_eq!(value_to_u64(Some(&serde_json::json!("43"))), Some(43));
        assert_eq!(value_to_u64(Some(&serde_json::json!(null))), None);
    }
}
