use context_contracts::{LocalApiCommand, LocalApiRequest};

#[test]
fn checked_in_text_read_fixture_has_no_client_selected_scope_or_grant() {
    let fixture = include_str!("../../../schemas/local-api/v1/read_text_content.json");
    let request: LocalApiRequest = serde_json::from_str(fixture).unwrap();

    let LocalApiCommand::ReadTextContent {
        authorization,
        event_id,
        sha256,
    } = &request.command
    else {
        panic!("fixture must be a read_text_content request");
    };
    assert_eq!(event_id.to_string(), "018c4a15-6f80-7000-8000-000000000202");
    assert_eq!(sha256, &"a".repeat(64));

    let expected: serde_json::Value = serde_json::from_str(fixture).unwrap();
    let actual = serde_json::to_value(&request).unwrap();
    assert_eq!(actual, expected);
    assert!(actual["command"].get("scope_id").is_none());
    assert!(actual["command"].get("retrieval_class").is_none());
    assert!(actual["command"].get("sensitivity").is_none());

    let debug = format!("{request:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains(&authorization.0));
}
