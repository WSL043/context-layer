from pathlib import Path

path = Path("apps/context-agent/src/read_capability.rs")
text = path.read_text(encoding="utf-8")

text = text.replace(
    '''pub(crate) struct ReadCapabilityPolicy {\n    token: Box<str>,\n    allowed_scopes: HashSet<String>,\n    grant: RetrievalGrant,\n}\n''',
    '''pub(crate) struct ReadCapabilityPolicy {\n    token: Box<str>,\n    allowed_scopes: HashSet<String>,\n    grant: RetrievalGrant,\n    allow_text_content: bool,\n}\n''',
    1,
)

old = '''        let grant = match profile.as_deref().unwrap_or("metadata") {\n            "metadata" => RetrievalGrant::metadata_only(),\n            "sensitive" => RetrievalGrant {\n                max_event_sensitivity: SensitivityClass::Sensitive,\n                max_content_retrieval: RetrievalClass::Sensitive,\n                include_payload: true,\n            },\n            other => {\n                return Err(format!(\n                    "unsupported {READ_PROFILE_ENV} value {other:?}; expected metadata or sensitive"\n                ));\n            }\n        };\n'''
new = '''        let (grant, allow_text_content) = match profile.as_deref().unwrap_or("metadata") {\n            "metadata" => (RetrievalGrant::metadata_only(), false),\n            "sensitive" => (\n                RetrievalGrant {\n                    max_event_sensitivity: SensitivityClass::Sensitive,\n                    max_content_retrieval: RetrievalClass::Sensitive,\n                    include_payload: true,\n                },\n                true,\n            ),\n            other => {\n                return Err(format!(\n                    "unsupported {READ_PROFILE_ENV} value {other:?}; expected metadata or sensitive"\n                ));\n            }\n        };\n'''
if old not in text and "let (grant, allow_text_content)" not in text:
    raise SystemExit("profile grant anchor mismatch")
text = text.replace(old, new, 1)

text = text.replace(
    '''        Ok(Some(Self {\n            token: token.into_boxed_str(),\n            allowed_scopes,\n            grant,\n        }))\n''',
    '''        Ok(Some(Self {\n            token: token.into_boxed_str(),\n            allowed_scopes,\n            grant,\n            allow_text_content,\n        }))\n''',
    1,
)

old = '''    #[cfg(test)]\n    pub(crate) fn for_test(token: &str, scopes: &[&str], grant: RetrievalGrant) -> Self {\n        Self {\n            token: token.into(),\n            allowed_scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),\n            grant,\n        }\n    }\n'''
new = '''    #[cfg(test)]\n    pub(crate) fn for_test(\n        token: &str,\n        scopes: &[&str],\n        grant: RetrievalGrant,\n        allow_text_content: bool,\n    ) -> Self {\n        Self {\n            token: token.into(),\n            allowed_scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),\n            grant,\n            allow_text_content,\n        }\n    }\n'''
if old not in text and "allow_text_content: bool," not in text.split("#[derive(Debug, PartialEq, Eq)]", 1)[0]:
    raise SystemExit("test policy constructor anchor mismatch")
text = text.replace(old, new, 1)

anchor = '''    pub(crate) fn scope_allowed(&self, scope_id: &ScopeId) -> bool {\n        self.allowed_scopes.contains(&scope_id.0)\n    }\n'''
if "pub(crate) fn text_content_allowed" not in text:
    if anchor not in text:
        raise SystemExit("scope_allowed anchor mismatch")
    text = text.replace(
        anchor,
        anchor
        + '''\n    pub(crate) fn text_content_allowed(&self) -> bool {\n        self.allow_text_content\n    }\n''',
        1,
    )

# Existing read-capability tests are about timeline grants; explicitly mark
# metadata constructors false and sensitive constructors true.
text = text.replace(
    '''            RetrievalGrant::metadata_only(),\n        );''',
    '''            RetrievalGrant::metadata_only(),\n            false,\n        );''',
)
text = text.replace(
    '''                include_payload: true,\n            },\n        );''',
    '''                include_payload: true,\n            },\n            true,\n        );''',
)
path.write_text(text, encoding="utf-8")

path = Path("apps/context-agent/src/content_read.rs")
text = path.read_text(encoding="utf-8")
needle = '''    let Some(grant) = policy.grant_for_token(authorization) else {\n        return Err(ContentReadError::NotAuthorized);\n    };\n'''
replacement = needle + '''    if !policy.text_content_allowed() {\n        return Err(ContentReadError::NotAuthorized);\n    }\n'''
if "if !policy.text_content_allowed()" not in text:
    if needle not in text:
        raise SystemExit("content policy gate anchor mismatch")
    text = text.replace(needle, replacement, 1)

# Sensitive helper gets the explicit byte-read bit.
text = text.replace(
    '''                include_payload: true,\n            },\n        )\n    }''',
    '''                include_payload: true,\n            },\n            true,\n        )\n    }''',
    1,
)

# Add a direct regression: metadata profile cannot read raw bytes even from a
# Metadata event with a Normal content reference.
if "metadata_profile_never_reads_raw_text_bytes" not in text:
    anchor = '''    #[test]\n    fn exact_event_and_digest_can_read_verified_sensitive_text() {'''
    test = '''    #[test]\n    fn metadata_profile_never_reads_raw_text_bytes() {\n        let (root, vault) = temp_vault();\n        let stored = vault.put_bytes(b"normal raw content").unwrap();\n        let reference = ContentRef {\n            sha256: stored.sha256.clone(),\n            media_type: "text/plain; charset=utf-8".into(),\n            byte_length: stored.byte_length,\n            compression: None,\n            storage_class: "local_vault".into(),\n            retrieval_class: RetrievalClass::Normal,\n        };\n        let repository = SqliteRepository::in_memory().unwrap();\n        let mut engine = ContextEngine::new(repository);\n        let mut event = event_with_ref(reference, "scope.personal");\n        event.sensitivity = SensitivityClass::Metadata;\n        engine.ingest_v2(&event).unwrap();\n        let metadata_policy = ReadCapabilityPolicy::for_test(\n            TOKEN,\n            &["scope.personal"],\n            RetrievalGrant::metadata_only(),\n            false,\n        );\n\n        assert_eq!(\n            read_text_content_with_policy(\n                engine.repository(),\n                &vault,\n                &metadata_policy,\n                &ReadCapabilityToken(TOKEN.into()),\n                event.event_id,\n                &stored.sha256,\n            )\n            .unwrap_err(),\n            ContentReadError::NotAuthorized\n        );\n        fs::remove_dir_all(root).unwrap();\n    }\n\n'''
    if anchor not in text:
        raise SystemExit("content test anchor mismatch")
    text = text.replace(anchor, test + anchor, 1)
path.write_text(text, encoding="utf-8")

path = Path(".github/workflows/ci.yml")
text = path.read_text(encoding="utf-8")
marker = "\n  normalize-content-read-profile:\n"
if marker in text:
    text = text.split(marker, 1)[0].rstrip() + "\n"
path.write_text(text, encoding="utf-8")
