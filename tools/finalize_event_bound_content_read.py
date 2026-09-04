from pathlib import Path

# Core module export.
path = Path("crates/core/src/lib.rs")
text = path.read_text(encoding="utf-8")
if "pub mod content_access;" not in text:
    text = text.replace("pub mod retrieval;", "pub mod content_access;\npub mod retrieval;", 1)
path.write_text(text, encoding="utf-8")

# SQLite module export. `use uuid::Uuid` also appears in tests, so only the
# first top-level import is the anchor.
path = Path("crates/storage-sqlite/src/lib.rs")
text = path.read_text(encoding="utf-8")
if "mod content_access;" not in text:
    anchor = "use uuid::Uuid;\n"
    if anchor not in text:
        raise SystemExit("storage import anchor missing")
    text = text.replace(anchor, anchor + "\nmod content_access;\n", 1)
path.write_text(text, encoding="utf-8")

# Contracts: wire DTO + command/result variants.
path = Path("crates/contracts/src/lib.rs")
text = path.read_text(encoding="utf-8")
if "pub struct LocalTextContent" not in text:
    anchor = '''#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]\npub struct LocalTimelinePage {\n    pub entries: Vec<LocalTimelineEntry>,\n    pub next_cursor: Option<LocalTimelineCursor>,\n}\n'''
    addition = anchor + '''\n#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]\npub struct LocalTextContent {\n    pub event_id: Uuid,\n    pub sha256: String,\n    pub media_type: String,\n    pub byte_length: u64,\n    pub text: String,\n}\n'''
    if text.count(anchor) != 1:
        raise SystemExit("LocalTimelinePage anchor mismatch")
    text = text.replace(anchor, addition, 1)
if "ReadTextContent {" not in text:
    anchor = '''    QueryTimeline {\n        authorization: ReadCapabilityToken,\n        query: LocalTimelineQuery,\n    },\n'''
    replacement = anchor + '''    ReadTextContent {\n        authorization: ReadCapabilityToken,\n        event_id: Uuid,\n        sha256: String,\n    },\n'''
    if text.count(anchor) != 1:
        raise SystemExit("QueryTimeline variant anchor mismatch")
    text = text.replace(anchor, replacement, 1)
if "TextContent { content: LocalTextContent }" not in text:
    anchor = '''    TimelinePage { page: LocalTimelinePage },\n'''
    if text.count(anchor) != 1:
        raise SystemExit("TimelinePage result anchor mismatch")
    text = text.replace(anchor, anchor + "    TextContent { content: LocalTextContent },\n", 1)
path.write_text(text, encoding="utf-8")

# Read capability: expose immutable server policy primitives to content-read path.
path = Path("apps/context-agent/src/read_capability.rs")
text = path.read_text(encoding="utf-8")
text = text.replace("pub struct ReadCapabilityPolicy {", "pub(crate) struct ReadCapabilityPolicy {", 1)
text = text.replace(
    '''    #[cfg(test)]\n    fn for_test(token: &str, scopes: &[&str], grant: RetrievalGrant) -> Self {''',
    '''    #[cfg(test)]\n    pub(crate) fn for_test(token: &str, scopes: &[&str], grant: RetrievalGrant) -> Self {''',
    1,
)
old_authorize = '''    fn authorize(&self, token: &ReadCapabilityToken, scope_id: &ScopeId) -> Option<RetrievalGrant> {\n        if !self.allowed_scopes.contains(&scope_id.0) {\n            return None;\n        }\n        constant_time_equal(self.token.as_bytes(), token.0.as_bytes()).then_some(self.grant)\n    }\n'''
new_authorize = '''    pub(crate) fn grant_for_token(\n        &self,\n        token: &ReadCapabilityToken,\n    ) -> Option<RetrievalGrant> {\n        constant_time_equal(self.token.as_bytes(), token.0.as_bytes()).then_some(self.grant)\n    }\n\n    pub(crate) fn scope_allowed(&self, scope_id: &ScopeId) -> bool {\n        self.allowed_scopes.contains(&scope_id.0)\n    }\n\n    fn authorize(&self, token: &ReadCapabilityToken, scope_id: &ScopeId) -> Option<RetrievalGrant> {\n        if !self.scope_allowed(scope_id) {\n            return None;\n        }\n        self.grant_for_token(token)\n    }\n'''
if old_authorize in text:
    text = text.replace(old_authorize, new_authorize, 1)
elif "pub(crate) fn grant_for_token" not in text:
    raise SystemExit("read capability authorize anchor mismatch")
old_env = '''pub fn query_timeline_from_environment(\n    repository: &SqliteRepository,\n    authorization: &ReadCapabilityToken,\n    query: LocalTimelineQuery,\n) -> Result<LocalTimelinePage, ReadRequestError> {\n    let policy = ENVIRONMENT_POLICY.get_or_init(ReadCapabilityPolicy::from_environment);\n    match policy {\n        Ok(Some(policy)) => query_timeline_with_policy(repository, policy, authorization, query),\n        Ok(None) => Err(ReadRequestError::NotAuthorized),\n        Err(error) => Err(ReadRequestError::Configuration(error.clone())),\n    }\n}\n'''
new_env = '''pub(crate) fn environment_read_policy(\n) -> Result<Option<&'static ReadCapabilityPolicy>, String> {\n    match ENVIRONMENT_POLICY.get_or_init(ReadCapabilityPolicy::from_environment) {\n        Ok(Some(policy)) => Ok(Some(policy)),\n        Ok(None) => Ok(None),\n        Err(error) => Err(error.clone()),\n    }\n}\n\npub fn query_timeline_from_environment(\n    repository: &SqliteRepository,\n    authorization: &ReadCapabilityToken,\n    query: LocalTimelineQuery,\n) -> Result<LocalTimelinePage, ReadRequestError> {\n    match environment_read_policy() {\n        Ok(Some(policy)) => query_timeline_with_policy(repository, policy, authorization, query),\n        Ok(None) => Err(ReadRequestError::NotAuthorized),\n        Err(error) => Err(ReadRequestError::Configuration(error)),\n    }\n}\n'''
if old_env in text:
    text = text.replace(old_env, new_env, 1)
elif "pub(crate) fn environment_read_policy" not in text:
    raise SystemExit("environment read policy anchor mismatch")
path.write_text(text, encoding="utf-8")

# Harden digest/media-type boundary in content read.
path = Path("apps/context-agent/src/content_read.rs")
text = path.read_text(encoding="utf-8")
if "const MAX_MEDIA_TYPE_BYTES" not in text:
    text = text.replace(
        "const MAX_TEXT_CONTENT_BYTES: usize = 96 * 1024;",
        "const MAX_TEXT_CONTENT_BYTES: usize = 96 * 1024;\nconst MAX_MEDIA_TYPE_BYTES: usize = 128;",
        1,
    )
needle = '''    let Some(grant) = policy.grant_for_token(authorization) else {\n        return Err(ContentReadError::NotAuthorized);\n    };\n'''
replacement = needle + '''    if !is_lowercase_sha256(sha256) {\n        return Err(ContentReadError::NotAuthorized);\n    }\n'''
if "if !is_lowercase_sha256(sha256)" not in text:
    if text.count(needle) != 1:
        raise SystemExit("content digest validation anchor mismatch")
    text = text.replace(needle, replacement, 1)
text = text.replace(
    '''    if reference.storage_class != "local_vault"\n        || reference.compression.is_some()\n        || !is_utf8_plain_text(&reference.media_type)\n''',
    '''    if reference.storage_class != "local_vault"\n        || reference.compression.is_some()\n        || reference.media_type.len() > MAX_MEDIA_TYPE_BYTES\n        || !is_utf8_plain_text(&reference.media_type)\n''',
    1,
)
if "fn is_lowercase_sha256" not in text:
    anchor = "fn is_utf8_plain_text(media_type: &str) -> bool {"
    helper = '''fn is_lowercase_sha256(value: &str) -> bool {\n    value.len() == 64\n        && value\n            .bytes()\n            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))\n}\n\n'''
    if text.count(anchor) != 1:
        raise SystemExit("media type helper anchor mismatch")
    text = text.replace(anchor, helper + anchor, 1)
path.write_text(text, encoding="utf-8")

# Agent composition and Local API dispatch.
path = Path("apps/context-agent/src/main.rs")
text = path.read_text(encoding="utf-8")
text = text.replace(
    "use std::{env, path::PathBuf};",
    "use std::{\n    env,\n    path::{Path, PathBuf},\n};",
    1,
)
if "use context_content_vault::ContentVault;" not in text:
    text = text.replace(
        "use anyhow::{Context, Result, bail};",
        "use anyhow::{Context, Result, bail};\nuse context_content_vault::ContentVault;",
        1,
    )
if "mod content_read;" not in text:
    text = text.replace("mod collector;", "mod collector;\nmod content_read;", 1)
if "fn open_content_vault_for_database" not in text:
    anchor = "fn serve_once(database_path: PathBuf) -> Result<()> {"
    helper = '''fn open_content_vault_for_database(database_path: &Path) -> Result<ContentVault> {\n    let data_root = database_path\n        .parent()\n        .filter(|parent| !parent.as_os_str().is_empty())\n        .unwrap_or_else(|| Path::new("."));\n    let vault_root = data_root.join("vault").join("blobs");\n    ContentVault::open(&vault_root)\n        .with_context(|| format!("open content vault {}", vault_root.display()))\n}\n\n'''
    if text.count(anchor) != 1:
        raise SystemExit("serve_once anchor mismatch")
    text = text.replace(anchor, helper + anchor, 1)
serve_old = '''    let mut engine = ContextEngine::new(repository);\n    let server = NamedPipeServer::bind_current_user().context("bind current-user named pipe")?;'''
serve_new = '''    let mut engine = ContextEngine::new(repository);\n    let content_vault = open_content_vault_for_database(&database_path)?;\n    let server = NamedPipeServer::bind_current_user().context("bind current-user named pipe")?;'''
serve_idx = text.find("fn serve_once(database_path: PathBuf)")
if serve_idx == -1:
    raise SystemExit("serve_once missing")
head, tail = text[:serve_idx], text[serve_idx:]
if "let content_vault = open_content_vault_for_database" not in tail.split("fn handle_request", 1)[0]:
    if serve_old not in tail:
        raise SystemExit("serve engine anchor mismatch")
    tail = tail.replace(serve_old, serve_new, 1)
text = head + tail
text = text.replace(
    "    let response = handle_request(&mut engine, request);",
    "    let response = handle_request(&mut engine, Some(&content_vault), request);",
    1,
)
old_sig = '''fn handle_request(\n    engine: &mut ContextEngine<SqliteRepository>,\n    request: LocalApiRequest,\n) -> LocalApiResponse {'''
new_sig = '''fn handle_request(\n    engine: &mut ContextEngine<SqliteRepository>,\n    content_vault: Option<&ContentVault>,\n    request: LocalApiRequest,\n) -> LocalApiResponse {'''
if old_sig in text:
    text = text.replace(old_sig, new_sig, 1)
elif "content_vault: Option<&ContentVault>" not in text:
    raise SystemExit("handle_request signature anchor mismatch")
if "LocalApiCommand::ReadTextContent" not in text:
    arm_anchor = '''            LocalApiCommand::QueryTimeline {\n                authorization,\n                query,\n            } => match read_capability::query_timeline_from_environment(\n                engine.repository(),\n                &authorization,\n                query,\n            ) {\n                Ok(page) => LocalApiResult::TimelinePage { page },\n                Err(error) => LocalApiResult::Error {\n                    code: error.code().into(),\n                    message: error.message(),\n                },\n            },\n'''
    arm_new = arm_anchor + '''            LocalApiCommand::ReadTextContent {\n                authorization,\n                event_id,\n                sha256,\n            } => match content_vault {\n                Some(vault) => match content_read::read_text_content_from_environment(\n                    engine.repository(),\n                    vault,\n                    &authorization,\n                    event_id,\n                    &sha256,\n                ) {\n                    Ok(content) => LocalApiResult::TextContent { content },\n                    Err(error) => LocalApiResult::Error {\n                        code: error.code().into(),\n                        message: error.message(),\n                    },\n                },\n                None => LocalApiResult::Error {\n                    code: "content_unavailable".into(),\n                    message: "content vault is unavailable in this runtime".into(),\n                },\n            },\n'''
    if text.count(arm_anchor) != 1:
        raise SystemExit("QueryTimeline dispatch anchor mismatch")
    text = text.replace(arm_anchor, arm_new, 1)
text = text.replace(
    "handle_request(\n            &mut engine,\n            LocalApiRequest",
    "handle_request(\n            &mut engine,\n            None,\n            LocalApiRequest",
)
path.write_text(text, encoding="utf-8")

# Runtime passes its already-open vault to API dispatch.
path = Path("apps/context-agent/src/runtime.rs")
text = path.read_text(encoding="utf-8")
text = text.replace(
    "let reply = handle_request(state.engine_mut(), request);",
    "let reply = handle_request(state.engine_mut(), _content_vault, request);",
    1,
)
path.write_text(text, encoding="utf-8")

# Remove this normalizer job from normal CI after it runs.
path = Path(".github/workflows/ci.yml")
text = path.read_text(encoding="utf-8")
marker = "\n  normalize-event-bound-content-read:\n"
if marker in text:
    text = text.split(marker, 1)[0].rstrip() + "\n"
path.write_text(text, encoding="utf-8")
