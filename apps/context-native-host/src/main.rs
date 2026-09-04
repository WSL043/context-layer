use std::{env, io};

use anyhow::{Context, Result, bail};
use context_contracts::{
    EventEnvelope, EventEnvelopeV2, EventPayload, LOCAL_API_VERSION, LocalApiCommand,
    LocalApiRequest, LocalApiResponse, SensitivityClass,
};
use context_local_ipc::{NamedPipeClient, read_frame, write_frame};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

const BROWSER_PROTOCOL_V1: u16 = 1;
const BROWSER_PROTOCOL_V2: u16 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrowserMessage {
    DownloadCompleted {
        protocol_version: u16,
        browser: String,
        browser_download_id: u64,
        source_sequence: u64,
        download_id: Uuid,
        url: String,
        referrer: Option<String>,
        final_path: String,
        #[serde(with = "time::serde::rfc3339")]
        observed_at: OffsetDateTime,
    },
    ActivePageChanged {
        protocol_version: u16,
        browser: String,
        observation_id: Uuid,
        source_sequence: u64,
        tab_id: u64,
        window_id: u64,
        url: String,
        title: String,
        pinned: bool,
        window_focused: bool,
        trigger: String,
        #[serde(with = "time::serde::rfc3339")]
        observed_at: OffsetDateTime,
    },
    CollectorGap {
        protocol_version: u16,
        browser: String,
        gap_id: Uuid,
        last_source_sequence: Option<u64>,
        reason: String,
        #[serde(with = "time::serde::rfc3339")]
        observed_at: OffsetDateTime,
    },
}

#[derive(Debug, Deserialize)]
struct NativeHostConfig {
    allowed_origins: Vec<String>,
}

fn main() -> Result<()> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|value| value == "--self-check")
    {
        return self_check();
    }
    if arguments
        .first()
        .is_some_and(|value| value == "--agent-self-check")
    {
        return agent_self_check(arguments.get(1).map(String::as_str));
    }

    let origin = arguments
        .iter()
        .find(|value| value.starts_with("chrome-extension://"))
        .context("browser extension origin argument is required")?;
    let allowed_origins = load_allowed_origins()?;
    if !allowed_origins
        .iter()
        .any(|allowed| origin.trim_end_matches('/') == allowed.trim_end_matches('/'))
    {
        bail!("browser extension origin is not allowlisted");
    }

    let message: BrowserMessage = read_frame(&mut io::stdin().lock())?;
    let (bridge_protocol_version, request) = request_from_browser(message)?;
    let mut pipe = NamedPipeClient::connect_current_user(5_000)
        .context("connect to the current-user context agent")?;
    write_frame(&mut pipe, &request)?;
    let mut response: LocalApiResponse = read_frame(&mut pipe)?;
    // The browser bridge and local agent API version independently. Reusing the
    // response shape is convenient, but the extension must receive its own bridge version.
    response.protocol_version = bridge_protocol_version;
    write_frame(&mut io::stdout().lock(), &response)?;
    Ok(())
}

fn load_allowed_origins() -> Result<Vec<String>> {
    if let Ok(origin) = env::var("CONTEXT_LAYER_ALLOWED_EXTENSION_ORIGIN") {
        return Ok(vec![origin]);
    }
    let executable = env::current_exe().context("resolve Native Host executable path")?;
    let config_path = executable
        .parent()
        .context("Native Host executable has no parent directory")?
        .join("native-host-allowlist.json");
    let config: NativeHostConfig = serde_json::from_reader(
        std::fs::File::open(&config_path)
            .with_context(|| format!("open Native Host config {}", config_path.display()))?,
    )
    .with_context(|| format!("parse Native Host config {}", config_path.display()))?;
    if config.allowed_origins.is_empty() {
        bail!("Native Host allowlist cannot be empty");
    }
    Ok(config.allowed_origins)
}

fn request_from_browser(message: BrowserMessage) -> Result<(u16, LocalApiRequest)> {
    let (protocol_version, browser) = match &message {
        BrowserMessage::DownloadCompleted {
            protocol_version,
            browser,
            ..
        }
        | BrowserMessage::ActivePageChanged {
            protocol_version,
            browser,
            ..
        }
        | BrowserMessage::CollectorGap {
            protocol_version,
            browser,
            ..
        } => (*protocol_version, browser.to_ascii_lowercase()),
    };

    if !matches!(protocol_version, BROWSER_PROTOCOL_V1 | BROWSER_PROTOCOL_V2) {
        bail!(
            "unsupported browser protocol version {}; supported versions are {} and {}",
            protocol_version,
            BROWSER_PROTOCOL_V1,
            BROWSER_PROTOCOL_V2
        );
    }
    if matches!(&message, BrowserMessage::ActivePageChanged { .. })
        && protocol_version != BROWSER_PROTOCOL_V2
    {
        bail!("active-page observations require browser protocol version 2");
    }
    if !matches!(browser.as_str(), "chrome" | "edge" | "chromium") {
        bail!("browser must be chrome, edge, or chromium");
    }

    let command = match message {
        BrowserMessage::DownloadCompleted {
            source_sequence,
            download_id,
            url,
            referrer,
            final_path,
            observed_at,
            ..
        } => {
            validate_web_url(&url).context("invalid download URL")?;
            if let Some(referrer) = &referrer {
                validate_web_url(referrer).context("invalid referrer URL")?;
            }
            if !is_absolute_windows_path(&final_path) {
                bail!("download final_path must be an absolute Windows path");
            }
            let scope = if protocol_version == BROWSER_PROTOCOL_V1 {
                "scope.downloads"
            } else {
                "scope.personal"
            };
            let mut event = EventEnvelope::observed(
                format!("browser.{browser}"),
                scope,
                observed_at,
                EventPayload::BrowserDownloadObserved {
                    download_id,
                    url,
                    referrer,
                    final_path,
                },
                "context-native-host",
                "browser downloads API through allowlisted native messaging origin",
            );
            event.event_id = download_id;
            event.source_sequence = Some(source_sequence);
            LocalApiCommand::SubmitEvent {
                event: Box::new(event),
            }
        }
        BrowserMessage::ActivePageChanged {
            observation_id,
            source_sequence,
            tab_id,
            window_id,
            url,
            title,
            pinned,
            window_focused,
            trigger,
            observed_at,
            ..
        } => {
            validate_web_url(&url).context("invalid active-page URL")?;
            if title.len() > 4096 {
                bail!("active-page title must be at most 4096 bytes");
            }
            if !matches!(
                trigger.as_str(),
                "startup"
                    | "installed"
                    | "tab_activated"
                    | "page_updated"
                    | "window_focused"
                    | "window_blurred"
            ) {
                bail!("unsupported active-page trigger");
            }
            let mut event = EventEnvelopeV2::observed(
                "browser.active_page_changed",
                format!("browser.{browser}"),
                "scope.personal",
                observed_at,
                json!({
                    "browser": browser,
                    "tab_id": tab_id,
                    "window_id": window_id,
                    "url": url,
                    "title": title,
                    "pinned": pinned,
                    "window_focused": window_focused,
                    "trigger": trigger,
                }),
                "context-native-host",
                "Chromium tabs/windows active-page state through allowlisted native messaging origin",
            );
            event.event_id = observation_id;
            event.source_sequence = Some(source_sequence);
            event.device_id = env::var("COMPUTERNAME").ok();
            event.sensitivity = SensitivityClass::Sensitive;
            LocalApiCommand::SubmitEventV2 {
                event: Box::new(event),
            }
        }
        BrowserMessage::CollectorGap {
            gap_id,
            last_source_sequence,
            reason,
            observed_at,
            ..
        } => {
            if reason.is_empty() || reason.len() > 1024 {
                bail!("collector gap reason must contain 1 to 1024 bytes");
            }
            let scope = if protocol_version == BROWSER_PROTOCOL_V1 {
                "scope.downloads"
            } else {
                "scope.personal"
            };
            let mut event = EventEnvelope::observed(
                format!("browser.{browser}"),
                scope,
                observed_at,
                EventPayload::CollectorGap {
                    collector: format!("browser.{browser}"),
                    last_sequence: last_source_sequence,
                    reason,
                },
                "context-native-host",
                "browser extension durable outbox reported a collection gap",
            );
            event.event_id = gap_id;
            event.source_sequence = last_source_sequence;
            LocalApiCommand::SubmitEvent {
                event: Box::new(event),
            }
        }
    };

    Ok((
        protocol_version,
        LocalApiRequest {
            request_id: Uuid::now_v7(),
            protocol_version: LOCAL_API_VERSION,
            command,
        },
    ))
}

fn validate_web_url(value: &str) -> Result<()> {
    if value.len() > 16_384 {
        bail!("URL must be at most 16384 bytes");
    }
    let has_web_prefix = value
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || value
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"));
    let parsed = Url::parse(value)?;
    if !has_web_prefix
        || !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
    {
        bail!("URL must use http or https and include a host");
    }
    Ok(())
}

fn is_absolute_windows_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    let drive_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    let unc_absolute =
        value.starts_with(r"\\") && (value[2..].contains('\\') || value[2..].contains('/'));
    drive_absolute || unc_absolute
}

fn self_check() -> Result<()> {
    let message = fixture_message(None);
    let mut framed = Vec::new();
    write_frame(&mut framed, &message)?;
    let decoded: BrowserMessage = read_frame(&mut framed.as_slice())?;
    let (bridge_version, request) = request_from_browser(decoded)?;
    assert_eq!(bridge_version, BROWSER_PROTOCOL_V2);
    assert!(matches!(
        request.command,
        LocalApiCommand::SubmitEvent { .. }
    ));
    println!("native-host self-check: framing and event validation passed");
    Ok(())
}

fn agent_self_check(final_path: Option<&str>) -> Result<()> {
    let (_, request) = request_from_browser(fixture_message(final_path))?;
    let mut pipe = NamedPipeClient::connect_current_user(5_000)
        .context("connect to the current-user context agent")?;
    write_frame(&mut pipe, &request)?;
    let response: LocalApiResponse = read_frame(&mut pipe)?;
    if !matches!(
        response.result,
        context_contracts::LocalApiResult::EventAccepted {
            duplicate: false,
            ..
        }
    ) {
        bail!(
            "agent rejected native host self-check: {:?}",
            response.result
        );
    }
    println!("native-host agent self-check: event accepted over named pipe");
    Ok(())
}

fn fixture_message(final_path: Option<&str>) -> BrowserMessage {
    BrowserMessage::DownloadCompleted {
        protocol_version: BROWSER_PROTOCOL_V2,
        browser: "edge".into(),
        browser_download_id: 42,
        source_sequence: 7,
        download_id: Uuid::now_v7(),
        url: "https://example.test/context-layer.pdf".into(),
        referrer: Some("https://example.test/".into()),
        final_path: final_path
            .unwrap_or(r"C:\Users\Example\Downloads\context-layer.pdf")
            .into(),
        observed_at: OffsetDateTime::now_utc(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(url: &str) -> BrowserMessage {
        BrowserMessage::DownloadCompleted {
            protocol_version: BROWSER_PROTOCOL_V2,
            browser: "edge".into(),
            browser_download_id: 42,
            source_sequence: 7,
            download_id: Uuid::now_v7(),
            url: url.into(),
            referrer: None,
            final_path: r"C:\Downloads\report.pdf".into(),
            observed_at: OffsetDateTime::now_utc(),
        }
    }

    fn active_page(protocol_version: u16, url: &str) -> BrowserMessage {
        BrowserMessage::ActivePageChanged {
            protocol_version,
            browser: "edge".into(),
            observation_id: Uuid::now_v7(),
            source_sequence: 8,
            tab_id: 12,
            window_id: 3,
            url: url.into(),
            title: "context layer - Search".into(),
            pinned: false,
            window_focused: true,
            trigger: "tab_activated".into(),
            observed_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn valid_download_becomes_a_versioned_submit_event() {
        let (bridge_version, request) =
            request_from_browser(message("https://example.test/report.pdf")).unwrap();
        assert_eq!(bridge_version, BROWSER_PROTOCOL_V2);
        assert_eq!(request.protocol_version, LOCAL_API_VERSION);
        assert!(matches!(
            request.command,
            LocalApiCommand::SubmitEvent { .. }
        ));
        let LocalApiCommand::SubmitEvent { event } = request.command else {
            unreachable!();
        };
        assert_eq!(event.scope_id.0, "scope.personal");
    }

    #[test]
    fn non_web_download_urls_are_rejected() {
        assert!(request_from_browser(message("file:///secret.txt")).is_err());
    }

    #[test]
    fn malformed_web_urls_are_rejected() {
        assert!(request_from_browser(message("https:missing-host")).is_err());
    }

    #[test]
    fn retrying_a_download_uses_the_download_uuid_as_event_id() {
        let message = message("https://example.test/report.pdf");
        let BrowserMessage::DownloadCompleted { download_id, .. } = &message else {
            unreachable!();
        };
        let expected = *download_id;
        let (_, request) = request_from_browser(message).unwrap();
        let LocalApiCommand::SubmitEvent { event } = request.command else {
            unreachable!();
        };
        assert_eq!(event.event_id, expected);
    }

    #[test]
    fn active_page_becomes_sensitive_v2_raw_evidence() {
        let message = active_page(
            BROWSER_PROTOCOL_V2,
            "https://www.google.com/search?q=context+layer",
        );
        let BrowserMessage::ActivePageChanged { observation_id, .. } = &message else {
            unreachable!();
        };
        let expected = *observation_id;
        let (bridge_version, request) = request_from_browser(message).unwrap();
        assert_eq!(bridge_version, BROWSER_PROTOCOL_V2);
        let LocalApiCommand::SubmitEventV2 { event } = request.command else {
            unreachable!();
        };
        assert_eq!(event.event_id, expected);
        assert_eq!(event.event_type, "browser.active_page_changed");
        assert_eq!(event.source_sequence, Some(8));
        assert_eq!(event.sensitivity, SensitivityClass::Sensitive);
        assert_eq!(
            event.payload["url"],
            "https://www.google.com/search?q=context+layer"
        );
        assert_eq!(event.payload["window_focused"], true);
    }

    #[test]
    fn active_page_requires_bridge_v2_and_web_url() {
        assert!(
            request_from_browser(active_page(
                BROWSER_PROTOCOL_V1,
                "https://example.test/"
            ))
            .is_err()
        );
        assert!(
            request_from_browser(active_page(BROWSER_PROTOCOL_V2, "chrome://settings/"))
                .is_err()
        );
    }

    #[test]
    fn browser_outbox_gap_becomes_a_collector_gap_event() {
        let (_, request) = request_from_browser(BrowserMessage::CollectorGap {
            protocol_version: BROWSER_PROTOCOL_V2,
            browser: "chromium".into(),
            gap_id: Uuid::now_v7(),
            last_source_sequence: Some(99),
            reason: "outbox capacity exceeded".into(),
            observed_at: OffsetDateTime::now_utc(),
        })
        .unwrap();
        let LocalApiCommand::SubmitEvent { event } = request.command else {
            unreachable!();
        };
        assert!(matches!(event.payload, EventPayload::CollectorGap { .. }));
        assert_eq!(event.source_sequence, Some(99));
        assert_eq!(event.scope_id.0, "scope.personal");
    }

    #[test]
    fn checked_in_browser_v1_fixture_remains_accepted() {
        let fixture = include_str!("../../../schemas/browser/v1/download_completed.json");
        let message: BrowserMessage = serde_json::from_str(fixture).unwrap();
        let (bridge_version, request) = request_from_browser(message).unwrap();
        assert_eq!(bridge_version, BROWSER_PROTOCOL_V1);
        let LocalApiCommand::SubmitEvent { event } = request.command else {
            unreachable!();
        };
        assert_eq!(
            event.event_id,
            Uuid::parse_str("018bcfe5-6800-7000-8000-000000000001").unwrap()
        );
        assert_eq!(event.source_sequence, Some(7));
        assert_eq!(event.scope_id.0, "scope.downloads");
    }

    #[test]
    fn checked_in_browser_v2_active_page_fixture_is_accepted() {
        let fixture = include_str!("../../../schemas/browser/v2/active_page_changed.json");
        let message: BrowserMessage = serde_json::from_str(fixture).unwrap();
        let (bridge_version, request) = request_from_browser(message).unwrap();
        assert_eq!(bridge_version, BROWSER_PROTOCOL_V2);
        let LocalApiCommand::SubmitEventV2 { event } = request.command else {
            unreachable!();
        };
        assert_eq!(
            event.event_id,
            Uuid::parse_str("018bcfe5-6800-7000-8000-000000000002").unwrap()
        );
        assert_eq!(event.payload["trigger"], "tab_activated");
    }
}
