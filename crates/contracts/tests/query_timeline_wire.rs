use context_contracts::{LocalApiCommand, LocalApiRequest, ScopeId};

#[test]
fn checked_in_timeline_query_fixture_round_trips_stably() {
    let fixture = include_str!("../../../schemas/local-api/v1/query_timeline.json");
    let request: LocalApiRequest = serde_json::from_str(fixture).unwrap();

    let LocalApiCommand::QueryTimeline {
        authorization,
        query,
    } = &request.command
    else {
        panic!("fixture must be a query_timeline request");
    };
    assert_eq!(authorization.0.len(), 48);
    assert_eq!(query.scope_id, ScopeId("scope.personal".into()));
    assert_eq!(query.limit, 10);

    let expected: serde_json::Value = serde_json::from_str(fixture).unwrap();
    let actual = serde_json::to_value(&request).unwrap();
    assert_eq!(actual, expected);

    let debug = format!("{request:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains(&authorization.0));
}
