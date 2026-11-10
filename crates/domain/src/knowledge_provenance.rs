use crate::{Error, Result, canonical_hash, valid_text};
use cowork_contracts::{KnowledgeOrigin, KnowledgeProvenance};
use std::collections::BTreeSet;

pub fn validate(provenance: &KnowledgeProvenance) -> Result<()> {
    let invalid = || Error::Validation("invalid_knowledge_provenance".into());
    if provenance.format != 1
        || provenance.sources.len() > 32
        || (matches!(
            provenance.origin,
            KnowledgeOrigin::Imported | KnowledgeOrigin::Retrieved
        ) && provenance.sources.is_empty())
        || (provenance.origin == KnowledgeOrigin::LegacyUnknown && !provenance.sources.is_empty())
    {
        return Err(invalid());
    }
    let mut seen = BTreeSet::new();
    for source in &provenance.sources {
        if source.format != 1
            || usize::from(source.source_uri.is_some())
                + usize::from(source.file_id.is_some())
                + usize::from(source.artifact_id.is_some())
                != 1
            || source.sha256.len() != 64
            || !source
                .sha256
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(invalid());
        }
        for (text, limit) in [(&source.revision, 200), (&source.locator, 1000)] {
            valid_text(text, limit)?;
            if text.chars().any(char::is_control) {
                return Err(invalid());
            }
        }
        if let Some(uri) = &source.source_uri {
            valid_text(uri, 2048)?;
            if uri.chars().any(char::is_control) {
                return Err(invalid());
            }
            if uri.starts_with("urn:cowork:") {
                if !uri
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._:/".contains(&b))
                {
                    return Err(invalid());
                }
            } else {
                let parsed = url::Url::parse(uri).map_err(|_| invalid())?;
                if parsed.scheme() != "https"
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || uri.contains('\\')
                    || uri.chars().any(char::is_whitespace)
                {
                    return Err(invalid());
                }
            }
        }
        for id in [&source.file_id, &source.artifact_id].into_iter().flatten() {
            valid_text(id, 128)?;
            if id.chars().any(char::is_control) {
                return Err(invalid());
            }
        }
        if !seen.insert(canonical_hash(source)) {
            return Err(invalid());
        }
    }
    Ok(())
}
