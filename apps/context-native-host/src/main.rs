use std::{env, io};

use anyhow::{Context, Result, bail};
use context_contracts::{
    EventEnvelope, EventPayload, LOCAL_API_VERSION, LocalApiCommand, LocalApiRequest,
    LocalApiResponse,
};
use context_local_ipc::{NamedPipeClient, read_frame, write_frame};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct BrowserDownloadMessage {
    protocol_version: u16,
    browser: String,
    download_id: Uuid,
    url: String,
    referrer: Option<String>,
    final_path: String,
    #[serde(with = "time::serde::rfc3339")]
    observed_at: OffsetDateTime,
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
        return agent_self_check();
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

    let message: BrowserDownloadMessage = read_frame(&mut io::stdin().lock())?;
    let request = request_from_browser(message)?;
    let mut pipe = NamedPipeClient::connect_current_user(5_000)
        .context("connect to the current-user context agent")?;
    write_frame(&mut pipe, &request)?;
    let response: LocalApiResponse = read_frame(&mut pipe)?;
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

fn request_from_browser(message: BrowserDownloadMessage) -> Result<LocalApiRequest> {
    if message.protocol_version != LOCAL_API_VERSION {
        bail!(
            "unsupported browser protocol version {}; expected {}",
            message.protocol_version,
            LOCAL_API_VERSION
        );
    }
    validate_web_url(&message.url).context("invalid download URL")?;
    if let Some(referrer) = &message.referrer {
        validate_web_url(referrer).context("invalid referrer URL")?;
    }
    if !is_absolute_windows_path(&message.final_path) {
        bail!("download final_path must be an absolute Windows path");
    }
    let browser = message.browser.to_ascii_lowercase();
    if !matches!(browser.as_str(), "chrome" | "edge" | "chromium") {
        bail!("browser must be chrome, edge, or chromium");
    }
    let event = EventEnvelope::observed(
        format!("browser.{browser}"),
        "scope.downloads",
        message.observed_at,
        EventPayload::BrowserDownloadObserved {
            download_id: message.download_id,
            url: message.url,
            referrer: message.referrer,
            final_path: message.final_path,
        },
        "context-native-host",
        "browser downloads API through allowlisted native messaging origin",
    );
    Ok(LocalApiRequest {
        request_id: Uuid::now_v7(),
        protocol_version: LOCAL_API_VERSION,
        command: LocalApiCommand::SubmitEvent {
            event: Box::new(event),
        },
    })
}

fn validate_web_url(value: &str) -> Result<()> {
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
    let message = fixture_message();
    let mut framed = Vec::new();
    write_frame(&mut framed, &message)?;
    let decoded: BrowserDownloadMessage = read_frame(&mut framed.as_slice())?;
    let request = request_from_browser(decoded)?;
    assert!(matches!(
        request.command,
        LocalApiCommand::SubmitEvent { .. }
    ));
    println!("native-host self-check: framing and event validation passed");
    Ok(())
}

fn agent_self_check() -> Result<()> {
    let request = request_from_browser(fixture_message())?;
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

fn fixture_message() -> BrowserDownloadMessage {
    BrowserDownloadMessage {
        protocol_version: LOCAL_API_VERSION,
        browser: "edge".into(),
        download_id: Uuid::now_v7(),
        url: "https://example.test/context-layer.pdf".into(),
        referrer: Some("https://example.test/".into()),
        final_path: r"C:\Users\Example\Downloads\context-layer.pdf".into(),
        observed_at: OffsetDateTime::now_utc(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(url: &str) -> BrowserDownloadMessage {
        BrowserDownloadMessage {
            protocol_version: LOCAL_API_VERSION,
            browser: "edge".into(),
            download_id: Uuid::now_v7(),
            url: url.into(),
            referrer: None,
            final_path: r"C:\Downloads\report.pdf".into(),
            observed_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn valid_download_becomes_a_versioned_submit_event() {
        let request = request_from_browser(message("https://example.test/report.pdf")).unwrap();
        assert_eq!(request.protocol_version, LOCAL_API_VERSION);
        assert!(matches!(
            request.command,
            LocalApiCommand::SubmitEvent { .. }
        ));
    }

    #[test]
    fn non_web_download_urls_are_rejected() {
        assert!(request_from_browser(message("file:///secret.txt")).is_err());
    }

    #[test]
    fn malformed_web_urls_are_rejected() {
        assert!(request_from_browser(message("https:missing-host")).is_err());
    }
}
