//! Native owner memory retains its source project and explicit retrieval scope.
use super::*;
pub(super) fn validate(metadata: &MemoryMetadata) -> Result<()> {
    valid_text(&metadata.category, 80)?;
    if metadata
        .confidence
        .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
    {
        return Err(Error::Validation(
            "Memory confidence must be between 0 and 1".into(),
        ));
    }
    if let Some(key) = &metadata.profile_key {
        valid_text(key, 128)?;
        if key.chars().any(char::is_control) {
            return Err(Error::Validation("Invalid memory profile key".into()));
        }
    }
    match &metadata.scope {
        MemoryScope::User | MemoryScope::Project => {}
        MemoryScope::Chat { order_id } => valid_text(order_id, 200)?,
        MemoryScope::Agent { agent_id } => valid_text(agent_id, 200)?,
    }
    Ok(())
}
pub(super) async fn bind(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    metadata: &MemoryMetadata,
) -> Result<()> {
    validate(metadata)?;
    if !metadata.enabled {
        return Ok(());
    }
    let valid=match &metadata.scope {
        MemoryScope::User|MemoryScope::Project=>true,
        MemoryScope::Chat{order_id}=>sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM orders WHERE id=$1 AND project_id=$2").bind(order_id).bind(project).fetch_one(&mut **tx).await.map_err(storage_error)?==1,
        MemoryScope::Agent{agent_id}=>sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM library_config_entries WHERE scope='installation-owner' AND kind='agent_template' AND id=$1 AND deleted=0").bind(agent_id).fetch_one(&mut **tx).await.map_err(storage_error)?==1,
    };
    if !valid {
        return Err(Error::Forbidden);
    }
    Ok(())
}
pub(super) fn applies(document: &KnowledgeDocument, spec: &RunSpec) -> bool {
    if document.project_id != spec.project_id
        && !document
            .memory
            .as_ref()
            .is_some_and(|m| m.scope == MemoryScope::User)
    {
        return false;
    }
    if document.kind != KnowledgeKind::Memory {
        return true;
    }
    let Some(metadata) = &document.memory else {
        return true;
    };
    if !metadata.enabled {
        return false;
    }
    match &metadata.scope {
        MemoryScope::User | MemoryScope::Project => true,
        MemoryScope::Chat { order_id } => order_id == &spec.order_id,
        MemoryScope::Agent { agent_id } => spec
            .agent_config
            .as_ref()
            .and_then(|v| v.agent_source.as_ref())
            .is_some_and(|source| source.id == *agent_id),
    }
}
pub(super) fn priority(document: &KnowledgeDocument) -> u8 {
    match document.memory.as_ref().map(|m| &m.scope) {
        Some(MemoryScope::Chat { .. }) => 3,
        Some(MemoryScope::Agent { .. }) => 2,
        Some(MemoryScope::User) => 0,
        _ => 1,
    }
}
pub(super) async fn index(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    document: &KnowledgeDocument,
) -> Result<()> {
    if document.kind == KnowledgeKind::Memory
        && document
            .memory
            .as_ref()
            .is_some_and(|m| m.enabled && m.scope == MemoryScope::User)
    {
        sqlx::query("INSERT INTO native_user_memories(document_id,project_id,revision) VALUES($1,$2,$3) ON CONFLICT(document_id) DO UPDATE SET project_id=excluded.project_id,revision=excluded.revision").bind(&document.id).bind(&document.project_id).bind(document.revision).execute(&mut **tx).await.map_err(storage_error)?;
    } else {
        sqlx::query("DELETE FROM native_user_memories WHERE document_id=$1")
            .bind(&document.id)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    Ok(())
}
