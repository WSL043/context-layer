from pathlib import Path

# Contracts: transient inline text parts for single-writer vault ingest.
path = Path("crates/contracts/src/lib.rs")
text = path.read_text(encoding="utf-8")
if "pub struct LocalSensitiveTextPart" not in text:
    anchor = '''#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]\npub struct LocalTextContent {\n    pub event_id: Uuid,\n    pub sha256: String,\n    pub media_type: String,\n    pub byte_length: u64,\n    pub text: String,\n}\n'''
    addition = anchor + '''\n#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]\npub struct LocalSensitiveTextPart {\n    pub role: String,\n    pub text: String,\n}\n'''
    if text.count(anchor) != 1:
        raise SystemExit("LocalTextContent anchor mismatch")
    text = text.replace(anchor, addition, 1)
if "SubmitSensitiveTextEventV2" not in text:
    anchor = '''    SubmitEventV2 {\n        event: Box<EventEnvelopeV2>,\n    },\n'''
    addition = anchor + '''    SubmitSensitiveTextEventV2 {\n        event: Box<EventEnvelopeV2>,\n        parts: Vec<LocalSensitiveTextPart>,\n    },\n'''
    if text.count(anchor) != 1:
        raise SystemExit("SubmitEventV2 anchor mismatch")
    text = text.replace(anchor, addition, 1)
path.write_text(text, encoding="utf-8")

# Agent module + dispatch into single-writer vault ingest.
path = Path("apps/context-agent/src/main.rs")
text = path.read_text(encoding="utf-8")
if "mod inline_content_ingest;" not in text:
    text = text.replace("mod content_read;", "mod content_read;\nmod inline_content_ingest;", 1)
if "LocalApiCommand::SubmitSensitiveTextEventV2" not in text:
    anchor = '''            LocalApiCommand::SubmitEventV2 { event } => match engine.ingest_v2(&event) {\n                Ok(report) => LocalApiResult::EventAccepted {\n                    event_id: event.event_id,\n                    duplicate: report.outcome == IngestOutcome::Duplicate,\n                },\n                Err(error) => LocalApiResult::Error {\n                    code: "ingest_failed".into(),\n                    message: error.to_string(),\n                },\n            },\n'''
    addition = anchor + '''            LocalApiCommand::SubmitSensitiveTextEventV2 { event, parts } => {\n                match inline_content_ingest::ingest_sensitive_text_event(\n                    engine,\n                    content_vault,\n                    *event,\n                    parts,\n                ) {\n                    Ok((event_id, duplicate)) => LocalApiResult::EventAccepted {\n                        event_id,\n                        duplicate,\n                    },\n                    Err(error) => LocalApiResult::Error {\n                        code: error.code().into(),\n                        message: error.message(),\n                    },\n                }\n            }\n'''
    if text.count(anchor) != 1:
        raise SystemExit("agent SubmitEventV2 dispatch anchor mismatch")
    text = text.replace(anchor, addition, 1)
path.write_text(text, encoding="utf-8")

# Native Host: browser bridge v3 + text interaction -> inline sensitive text command.
path = Path("apps/context-native-host/src/main.rs")
text = path.read_text(encoding="utf-8")
text = text.replace(
    '''    LocalApiRequest, LocalApiResponse, SensitivityClass,\n''',
    '''    LocalApiRequest, LocalApiResponse, LocalSensitiveTextPart, SensitivityClass,\n''',
    1,
)
if "const BROWSER_PROTOCOL_V3" not in text:
    text = text.replace(
        "const BROWSER_PROTOCOL_V2: u16 = 2;",
        "const BROWSER_PROTOCOL_V2: u16 = 2;\nconst BROWSER_PROTOCOL_V3: u16 = 3;\nconst MAX_SELECTED_TEXT_BYTES: usize = 64 * 1024;\nconst MAX_VISIBLE_CONTEXT_BYTES: usize = 16 * 1024;",
        1,
    )
if "TextInteractionObserved {" not in text.split("struct NativeHostConfig", 1)[0]:
    anchor = '''    CollectorGap {\n        protocol_version: u16,\n        browser: String,\n        gap_id: Uuid,\n'''
    variant = '''    TextInteractionObserved {\n        protocol_version: u16,\n        browser: String,\n        observation_id: Uuid,\n        source_sequence: u64,\n        tab_id: u64,\n        window_id: u64,\n        url: String,\n        title: String,\n        interaction: String,\n        selection_status: String,\n        selected_utf8_bytes: u64,\n        selected_text: Option<String>,\n        context_status: String,\n        visible_context: Option<String>,\n        #[serde(with = "time::serde::rfc3339")]\n        observed_at: OffsetDateTime,\n    },\n'''
    if anchor not in text:
        raise SystemExit("BrowserMessage collector gap anchor mismatch")
    text = text.replace(anchor, variant + anchor, 1)

# Browser protocol extraction must include the new variant.
old = '''        | BrowserMessage::ActivePageChanged {\n            protocol_version,\n            browser,\n            ..\n        }\n        | BrowserMessage::CollectorGap {'''
new = '''        | BrowserMessage::ActivePageChanged {\n            protocol_version,\n            browser,\n            ..\n        }\n        | BrowserMessage::TextInteractionObserved {\n            protocol_version,\n            browser,\n            ..\n        }\n        | BrowserMessage::CollectorGap {'''
if old in text:
    text = text.replace(old, new, 1)
elif "| BrowserMessage::TextInteractionObserved" not in text:
    raise SystemExit("browser protocol extraction anchor mismatch")

text = text.replace(
    '''    if !matches!(protocol_version, BROWSER_PROTOCOL_V1 | BROWSER_PROTOCOL_V2) {\n        bail!(\n            "unsupported browser protocol version {}; supported versions are {} and {}",\n            protocol_version,\n            BROWSER_PROTOCOL_V1,\n            BROWSER_PROTOCOL_V2\n        );\n    }\n''',
    '''    if !matches!(\n        protocol_version,\n        BROWSER_PROTOCOL_V1 | BROWSER_PROTOCOL_V2 | BROWSER_PROTOCOL_V3\n    ) {\n        bail!(\n            "unsupported browser protocol version {}; supported versions are {}, {}, and {}",\n            protocol_version,\n            BROWSER_PROTOCOL_V1,\n            BROWSER_PROTOCOL_V2,\n            BROWSER_PROTOCOL_V3\n        );\n    }\n''',
    1,
)
if "text-interaction observations require browser protocol version 3" not in text:
    anchor = '''    if matches!(&message, BrowserMessage::ActivePageChanged { .. })\n        && protocol_version != BROWSER_PROTOCOL_V2\n    {\n        bail!("active-page observations require browser protocol version 2");\n    }\n'''
    replacement = '''    if matches!(&message, BrowserMessage::ActivePageChanged { .. })\n        && protocol_version < BROWSER_PROTOCOL_V2\n    {\n        bail!("active-page observations require browser protocol version 2 or newer");\n    }\n    if matches!(&message, BrowserMessage::TextInteractionObserved { .. })\n        && protocol_version != BROWSER_PROTOCOL_V3\n    {\n        bail!("text-interaction observations require browser protocol version 3");\n    }\n'''
    if anchor not in text:
        raise SystemExit("active page protocol gate anchor mismatch")
    text = text.replace(anchor, replacement, 1)

# Add text-interaction mapping before CollectorGap match arm.
if "BrowserMessage::TextInteractionObserved {" not in text.split("let command = match message",1)[1].split("BrowserMessage::CollectorGap",1)[0]:
    anchor = '''        BrowserMessage::CollectorGap {\n            gap_id,\n'''
    arm = '''        BrowserMessage::TextInteractionObserved {\n            observation_id,\n            source_sequence,\n            tab_id,\n            window_id,\n            url,\n            title,\n            interaction,\n            selection_status,\n            selected_utf8_bytes,\n            selected_text,\n            context_status,\n            visible_context,\n            observed_at,\n            ..\n        } => {\n            validate_web_url(&url).context("invalid text-interaction URL")?;\n            if title.len() > 4096 {\n                bail!("text-interaction title must be at most 4096 bytes");\n            }\n            if !matches!(interaction.as_str(), "selection" | "copy") {\n                bail!("unsupported text interaction");\n            }\n            let selected_len = usize::try_from(selected_utf8_bytes)\n                .context("selected UTF-8 byte length does not fit this platform")?;\n            let selected_part = match selection_status.as_str() {\n                "retained" => {\n                    let text = selected_text\n                        .as_ref()\n                        .context("retained selection requires selected_text")?;\n                    if text.is_empty()\n                        || text.len() != selected_len\n                        || text.len() > MAX_SELECTED_TEXT_BYTES\n                    {\n                        bail!("retained selected text byte length is invalid");\n                    }\n                    Some(LocalSensitiveTextPart {\n                        role: "selected_text".into(),\n                        text: text.clone(),\n                    })\n                }\n                "omitted_too_large" => {\n                    if selected_text.is_some() || selected_len <= MAX_SELECTED_TEXT_BYTES {\n                        bail!("oversized selection must omit selected_text");\n                    }\n                    None\n                }\n                _ => bail!("unsupported selection status"),\n            };\n\n            let context_part = match context_status.as_str() {\n                "retained" => {\n                    let text = visible_context\n                        .as_ref()\n                        .context("retained context requires visible_context")?;\n                    if text.is_empty() || text.len() > MAX_VISIBLE_CONTEXT_BYTES {\n                        bail!("visible context byte length is invalid");\n                    }\n                    Some(LocalSensitiveTextPart {\n                        role: "visible_context".into(),\n                        text: text.clone(),\n                    })\n                }\n                "unavailable" | "omitted_too_large" => {\n                    if visible_context.is_some() {\n                        bail!("non-retained context must omit visible_context");\n                    }\n                    None\n                }\n                _ => bail!("unsupported context status"),\n            };\n\n            let event_type = if interaction == "copy" {\n                "browser.copy_observed"\n            } else {\n                "browser.text_selected"\n            };\n            let mut event = EventEnvelopeV2::observed(\n                event_type,\n                format!("browser.{browser}"),\n                "scope.personal",\n                observed_at,\n                json!({\n                    "browser": browser,\n                    "tab_id": tab_id,\n                    "window_id": window_id,\n                    "url": url,\n                    "title": title,\n                    "interaction": interaction,\n                    "selection_status": selection_status,\n                    "selected_utf8_bytes": selected_utf8_bytes,\n                    "context_status": context_status,\n                }),\n                "context-native-host",\n                "trusted Chromium selection/copy interaction through allowlisted native messaging origin",\n            );\n            event.event_id = observation_id;\n            event.source_sequence = Some(source_sequence);\n            event.device_id = env::var("COMPUTERNAME").ok();\n            event.sensitivity = SensitivityClass::Sensitive;\n\n            let mut parts = Vec::with_capacity(2);\n            if let Some(part) = selected_part {\n                parts.push(part);\n            }\n            if let Some(part) = context_part {\n                parts.push(part);\n            }\n            LocalApiCommand::SubmitSensitiveTextEventV2 {\n                event: Box::new(event),\n                parts,\n            }\n        }\n'''
    if anchor not in text:
        raise SystemExit("CollectorGap match arm anchor mismatch")
    text = text.replace(anchor, arm + anchor, 1)

# Update native-host self-check fixture to current v3 while retaining explicit v1/v2 tests.
text = text.replace(
    "assert_eq!(bridge_version, BROWSER_PROTOCOL_V2);",
    "assert_eq!(bridge_version, BROWSER_PROTOCOL_V3);",
    1,
)
text = text.replace(
    '''        protocol_version: BROWSER_PROTOCOL_V2,\n        browser: "edge".into(),\n        browser_download_id: 42,''',
    '''        protocol_version: BROWSER_PROTOCOL_V3,\n        browser: "edge".into(),\n        browser_download_id: 42,''',
    2,
)
text = text.replace(
    '''        assert_eq!(bridge_version, BROWSER_PROTOCOL_V2);\n        assert_eq!(request.protocol_version, LOCAL_API_VERSION);''',
    '''        assert_eq!(bridge_version, BROWSER_PROTOCOL_V3);\n        assert_eq!(request.protocol_version, LOCAL_API_VERSION);''',
    1,
)

# Add v3 fixture/validation tests at the end of the native-host test module.
if "checked_in_browser_v3_text_interaction_becomes_inline_sensitive_text" not in text:
    anchor = '''    #[test]\n    fn checked_in_browser_v2_active_page_fixture_is_accepted() {'''
    # Insert helper + tests before the existing final v2 fixture test so no brace surgery is needed.
    tests = '''    #[test]\n    fn checked_in_browser_v3_text_interaction_becomes_inline_sensitive_text() {\n        let fixture = include_str!("../../../schemas/browser/v3/text_interaction_observed.json");\n        let message: BrowserMessage = serde_json::from_str(fixture).unwrap();\n        let (bridge_version, request) = request_from_browser(message).unwrap();\n        assert_eq!(bridge_version, BROWSER_PROTOCOL_V3);\n        let LocalApiCommand::SubmitSensitiveTextEventV2 { event, parts } = request.command else {\n            unreachable!();\n        };\n        assert_eq!(event.event_type, "browser.copy_observed");\n        assert_eq!(event.source_sequence, Some(9));\n        assert_eq!(event.sensitivity, SensitivityClass::Sensitive);\n        assert!(event.content_refs.is_empty());\n        assert_eq!(parts.len(), 2);\n        assert_eq!(parts[0].role, "selected_text");\n        assert_eq!(parts[0].text, "selected context");\n        assert_eq!(parts[1].role, "visible_context");\n        assert!(!event.payload.to_string().contains("selected context"));\n    }\n\n    #[test]\n    fn text_interaction_requires_v3_and_consistent_body_status() {\n        let fixture = include_str!("../../../schemas/browser/v3/text_interaction_observed.json");\n        let mut message: BrowserMessage = serde_json::from_str(fixture).unwrap();\n        if let BrowserMessage::TextInteractionObserved { protocol_version, .. } = &mut message {\n            *protocol_version = BROWSER_PROTOCOL_V2;\n        }\n        assert!(request_from_browser(message).is_err());\n\n        let mut message: BrowserMessage = serde_json::from_str(fixture).unwrap();\n        if let BrowserMessage::TextInteractionObserved {\n            selected_utf8_bytes,\n            ..\n        } = &mut message\n        {\n            *selected_utf8_bytes += 1;\n        }\n        assert!(request_from_browser(message).is_err());\n    }\n\n'''
    if anchor not in text:
        raise SystemExit("native host v2 fixture test anchor mismatch")
    text = text.replace(anchor, tests + anchor, 1)
path.write_text(text, encoding="utf-8")

# CI: syntax-check the content script too. Remove this normalizer after running.
path = Path(".github/workflows/ci.yml")
text = path.read_text(encoding="utf-8")
check_anchor = "      - run: node --check apps/browser-extension/service-worker.js\n"
if "node --check apps/browser-extension/content-script.js" not in text:
    if check_anchor not in text:
        raise SystemExit("extension syntax check anchor mismatch")
    text = text.replace(
        check_anchor,
        check_anchor + "      - run: node --check apps/browser-extension/content-script.js\n",
        1,
    )
marker = "\n  normalize-browser-text-interactions:\n"
if marker in text:
    text = text.split(marker, 1)[0].rstrip() + "\n"
path.write_text(text, encoding="utf-8")
