//! Owner knowledge folders and topic-scoped, revision-bound graph retrieval.
use super::*;
use crate::productivity::{begin_productivity, product_project};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) async fn migrate(tx: &mut sqlx::Transaction<'_, sqlx::Any>) -> Result<()> {
    for statement in [
        "CREATE TABLE IF NOT EXISTS native_knowledge_folders(scope TEXT PRIMARY KEY, payload TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS native_knowledge_graphs(scope TEXT NOT NULL, folder_id TEXT NOT NULL, descendants BIGINT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(scope,folder_id,descendants))",
    ] {
        sqlx::query(statement)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    Ok(())
}

fn scope(project: &str, personal: bool) -> String {
    if personal {
        "owner-personal".into()
    } else {
        format!("project:{project}")
    }
}
pub(crate) fn personal(document: &KnowledgeDocument) -> bool {
    document.kind == KnowledgeKind::Memory
        && document
            .memory
            .as_ref()
            .is_some_and(|m| m.scope == MemoryScope::User)
}
pub(crate) fn validate_folders(folders: &[KnowledgeFolder]) -> Result<()> {
    if folders.len() > 500 {
        return Err(Error::Validation("knowledge_folder_limit".into()));
    }
    let mut ids = BTreeMap::new();
    let mut siblings = BTreeSet::new();
    for folder in folders {
        if Uuid::parse_str(&folder.id).is_err()
            || folder.name.trim() != folder.name
            || folder.name.is_empty()
            || folder.name.len() > 160
            || folder
                .name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
            || ids.insert(&folder.id, folder).is_some()
            || !siblings.insert((&folder.parent_id, folder.name.to_lowercase()))
        {
            return Err(Error::Validation("invalid_knowledge_folder".into()));
        }
    }
    for folder in folders {
        let mut seen = BTreeSet::new();
        let mut current = Some(&folder.id);
        while let Some(id) = current {
            if !seen.insert(id) || seen.len() > 12 {
                return Err(Error::Validation("knowledge_folder_cycle_or_depth".into()));
            }
            current = ids
                .get(id)
                .ok_or_else(|| Error::Validation("knowledge_folder_parent_missing".into()))?
                .parent_id
                .as_ref();
        }
    }
    Ok(())
}
pub(crate) async fn folders_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    is_personal: bool,
) -> Result<KnowledgeFolders> {
    let row = sqlx::query("SELECT payload FROM native_knowledge_folders WHERE scope=$1")
        .bind(scope(project, is_personal))
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?;
    row.map_or_else(
        || {
            Ok(KnowledgeFolders {
                revision: 0,
                folders: vec![],
            })
        },
        |r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?),
    )
}
pub(crate) async fn bind_location(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    is_personal: bool,
    folder: Option<&str>,
) -> Result<()> {
    if let Some(id) = folder
        && !folders_in_tx(tx, project, is_personal)
            .await?
            .folders
            .iter()
            .any(|f| f.id == id)
    {
        return Err(Error::Validation("knowledge_folder_missing".into()));
    }
    Ok(())
}

/// Archive folder identities never become references into an unrelated library.
/// Personal folders merge with fresh IDs; project replacement replaces its tree.
pub(crate) async fn import_folders(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    is_personal: bool,
    incoming: &[KnowledgeFolder],
) -> Result<BTreeMap<String, String>> {
    if is_personal && incoming.is_empty() {
        return Ok(BTreeMap::new());
    }
    let current = folders_in_tx(tx, project, is_personal).await?;
    let mapping: BTreeMap<_, _> = incoming.iter().map(|f| (f.id.clone(), id())).collect();
    let mut folders = if is_personal { current.folders } else { vec![] };
    for folder in incoming {
        let mut name = folder.name.clone();
        if is_personal
            && folder.parent_id.is_none()
            && folders
                .iter()
                .any(|f| f.parent_id.is_none() && f.name.to_lowercase() == name.to_lowercase())
        {
            while name.len() > 145 {
                name.pop();
            }
            name = format!("{} ({})", name, &mapping[&folder.id][..8]);
        }
        folders.push(KnowledgeFolder {
            id: mapping[&folder.id].clone(),
            name,
            parent_id: folder.parent_id.as_ref().map(|p| mapping[p].clone()),
        });
    }
    validate_folders(&folders)?;
    let result = KnowledgeFolders {
        revision: current.revision.checked_add(1).ok_or(Error::Conflict)?,
        folders,
    };
    sqlx::query("INSERT INTO native_knowledge_folders(scope,payload) VALUES($1,$2) ON CONFLICT(scope) DO UPDATE SET payload=excluded.payload").bind(scope(project,is_personal)).bind(json_string(&result)?).execute(&mut **tx).await.map_err(storage_error)?;
    if !is_personal {
        sqlx::query("DELETE FROM native_knowledge_graphs WHERE scope=$1")
            .bind(scope(project, false))
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    Ok(mapping)
}
async fn corpus(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    is_personal: bool,
) -> Result<Vec<KnowledgeDocument>> {
    let rows = if is_personal {
        // Payloads are always compact serde JSON. Include disabled personal entries
        // too: disabling memory must not make its containing folder deletable.
        // The exact scope object is checked again after deserialization below.
        sqlx::query("SELECT payload FROM productivity_objects WHERE kind='knowledge' AND project_id NOT IN(SELECT project_id FROM native_project_retirements) AND payload LIKE $1 ORDER BY id LIMIT 2001").bind("%\"scope\":{\"kind\":\"user\"}%").fetch_all(&mut **tx).await
    } else {
        sqlx::query("SELECT payload FROM productivity_objects WHERE kind='knowledge' AND project_id=$1 ORDER BY id LIMIT 2001").bind(project).fetch_all(&mut **tx).await
    }.map_err(storage_error)?;
    if rows.len() > 2000 {
        return Err(Error::Validation("knowledge_corpus_limit".into()));
    }
    let mut docs = Vec::new();
    for row in rows {
        let doc: KnowledgeDocument =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if personal(&doc) == is_personal {
            docs.push(doc);
        }
    }
    Ok(docs)
}
fn selected_folders(
    folders: &[KnowledgeFolder],
    root: &str,
    descendants: bool,
) -> Result<BTreeSet<String>> {
    if !folders.iter().any(|f| f.id == root) {
        return Err(Error::Validation("knowledge_folder_missing".into()));
    }
    let mut selected = BTreeSet::from([root.to_string()]);
    if descendants {
        loop {
            let before = selected.len();
            for folder in folders {
                if folder
                    .parent_id
                    .as_ref()
                    .is_some_and(|p| selected.contains(p))
                {
                    selected.insert(folder.id.clone());
                }
            }
            if before == selected.len() {
                break;
            }
        }
    }
    Ok(selected)
}
fn fingerprint(
    folders: &[KnowledgeFolder],
    docs: &[KnowledgeDocument],
    root: &str,
    descendants: bool,
) -> Result<String> {
    let selected = selected_folders(folders, root, descendants)?;
    let mut topics: Vec<_> = folders
        .iter()
        .filter(|f| selected.contains(&f.id))
        .collect();
    topics.sort_by(|a, b| a.id.cmp(&b.id));
    let mut revisions: Vec<_> = docs
        .iter()
        .filter(|d| d.folder_id.as_ref().is_some_and(|f| selected.contains(f)))
        .map(|d| (&d.project_id, &d.id, d.revision, &d.folder_id))
        .collect();
    revisions.sort();
    Ok(canonical_hash(&(
        "topic-graph-v1",
        root,
        descendants,
        topics,
        revisions,
    )))
}
fn document_node(id: &str) -> String {
    format!("document:{id}")
}
fn make_graph(
    folders: &[KnowledgeFolder],
    docs: &[KnowledgeDocument],
    input: &BuildKnowledgeGraph,
) -> Result<KnowledgeTopicGraph> {
    let selected = selected_folders(folders, &input.folder_id, input.include_descendants)?;
    let mut nodes = BTreeMap::new();
    let mut edges = Vec::new();
    for folder in folders.iter().filter(|f| selected.contains(&f.id)) {
        let id = format!("topic:{}", folder.id);
        nodes.insert(
            id.clone(),
            KnowledgeGraphNode {
                id: id.clone(),
                label: folder.name.clone(),
                kind: KnowledgeNodeKind::Topic,
            },
        );
        if let Some(parent) = &folder.parent_id
            && selected.contains(parent)
        {
            edges.push(KnowledgeGraphEdge {
                from: format!("topic:{parent}"),
                to: id,
                relation: "contains".into(),
            });
        }
    }
    let mut count = 0;
    for doc in docs
        .iter()
        .filter(|d| d.folder_id.as_ref().is_some_and(|f| selected.contains(f)))
    {
        count += 1;
        let id = document_node(&doc.id);
        nodes.insert(
            id.clone(),
            KnowledgeGraphNode {
                id: id.clone(),
                label: doc.title.clone(),
                kind: KnowledgeNodeKind::Document,
            },
        );
        edges.push(KnowledgeGraphEdge {
            from: format!("topic:{}", doc.folder_id.as_deref().unwrap_or_default()),
            to: id.clone(),
            relation: "contains".into(),
        });
        for tag in &doc.tags {
            let target = format!("tag:{}", tag.to_lowercase());
            nodes
                .entry(target.clone())
                .or_insert_with(|| KnowledgeGraphNode {
                    id: target.clone(),
                    label: tag.clone(),
                    kind: KnowledgeNodeKind::Tag,
                });
            edges.push(KnowledgeGraphEdge {
                from: id.clone(),
                to: target,
                relation: "tagged".into(),
            });
        }
        for source in &doc.provenance.sources {
            let target = format!(
                "source:{}",
                canonical_hash(&(
                    &source.source_uri,
                    &source.file_id,
                    &source.artifact_id,
                    &source.revision,
                    &source.sha256
                ))
            );
            nodes
                .entry(target.clone())
                .or_insert_with(|| KnowledgeGraphNode {
                    id: target.clone(),
                    label: source
                        .source_uri
                        .clone()
                        .or(source.file_id.clone())
                        .or(source.artifact_id.clone())
                        .unwrap_or_else(|| source.sha256.clone()),
                    kind: KnowledgeNodeKind::Source,
                });
            edges.push(KnowledgeGraphEdge {
                from: id.clone(),
                to: target,
                relation: "cites".into(),
            });
        }
    }
    if nodes.len() > 12000 || edges.len() > 24000 {
        return Err(Error::Validation("knowledge_graph_limit".into()));
    }
    Ok(KnowledgeTopicGraph {
        folder_id: input.folder_id.clone(),
        include_descendants: input.include_descendants,
        fingerprint: fingerprint(folders, docs, &input.folder_id, input.include_descendants)?,
        built_at: now(),
        stale: false,
        documents: count,
        nodes: nodes.into_values().collect(),
        edges,
    })
}

/// Lexical seeds, followed by one document-tag/source-document hop. Topic containment
/// bounds the corpus; merely sharing a folder does not fabricate a semantic link.
fn retrieve(
    graph: &KnowledgeTopicGraph,
    docs: &[KnowledgeDocument],
    query: &str,
) -> Vec<KnowledgeGraphHit> {
    let words: BTreeSet<_> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 2)
        .map(str::to_lowercase)
        .take(64)
        .collect();
    if words.is_empty() {
        return vec![];
    }
    let present: BTreeSet<_> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == KnowledgeNodeKind::Document)
        .map(|n| n.id.as_str())
        .collect();
    let mut scores = BTreeMap::<String, (usize, String)>::new();
    for doc in docs
        .iter()
        .filter(|d| present.contains(document_node(&d.id).as_str()))
    {
        let text = format!("{} {} {}", doc.title, doc.tags.join(" "), doc.content).to_lowercase();
        let score = words.iter().filter(|w| text.contains(w.as_str())).count() * 10;
        if score > 0 {
            scores.insert(doc.id.clone(), (score, "direct".into()));
        }
    }
    let mut seeds: Vec<_> = scores
        .iter()
        .map(|(id, (score, _))| (id.clone(), *score))
        .collect();
    seeds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let available: BTreeSet<_> = docs.iter().map(|d| document_node(&d.id)).collect();
    let mut neighbors = BTreeMap::<&str, Vec<&str>>::new();
    let mut links = BTreeMap::<&str, Vec<&str>>::new();
    for edge in graph.edges.iter().filter(|e| {
        matches!(e.relation.as_str(), "tagged" | "cites") && available.contains(&e.from)
    }) {
        neighbors.entry(&edge.to).or_default().push(&edge.from);
        links.entry(&edge.from).or_default().push(&edge.to);
    }
    for (seed, score) in seeds.into_iter().take(3) {
        if let Some(targets) = links.get(document_node(&seed).as_str()) {
            for target in targets {
                for neighbor in neighbors.get(target).into_iter().flatten() {
                    let id = neighbor.strip_prefix("document:").unwrap_or(neighbor);
                    scores
                        .entry(id.into())
                        .or_insert_with(|| ((score / 2).max(1), (*target).into()));
                }
            }
        }
    }
    let mut hits: Vec<_> = docs
        .iter()
        .filter_map(|doc| {
            scores.get(&doc.id).map(|(score, via)| KnowledgeGraphHit {
                document: doc.clone(),
                score: *score,
                via: via.clone(),
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.document.id.cmp(&b.document.id))
    });
    hits.truncate(5);
    hits
}

async fn graphs_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    is_personal: bool,
) -> Result<Vec<KnowledgeTopicGraph>> {
    let rows = sqlx::query("SELECT payload FROM native_knowledge_graphs WHERE scope=$1 ORDER BY folder_id,descendants LIMIT 33").bind(scope(project,is_personal)).fetch_all(&mut **tx).await.map_err(storage_error)?;
    rows.into_iter()
        .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
        .collect()
}

impl Store {
    pub async fn delete_knowledge_graph(
        &self,
        project: &str,
        is_personal: bool,
        input: BuildKnowledgeGraph,
    ) -> Result<()> {
        let mut tx = begin_productivity(self).await?;
        product_project(&mut tx, project).await?;
        sqlx::query("DELETE FROM native_knowledge_graphs WHERE scope=$1 AND folder_id=$2 AND descendants=$3").bind(scope(project,is_personal)).bind(input.folder_id).bind(i64::from(input.include_descendants)).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn knowledge_folders(
        &self,
        project: &str,
        is_personal: bool,
    ) -> Result<KnowledgeFolders> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        product_project(&mut tx, project).await?;
        folders_in_tx(&mut tx, project, is_personal).await
    }
    pub async fn write_knowledge_folders(
        &self,
        project: &str,
        is_personal: bool,
        input: WriteKnowledgeFolders,
    ) -> Result<KnowledgeFolders> {
        if input.expected_revision < 0 {
            return Err(Error::Validation(
                "invalid_knowledge_folder_revision".into(),
            ));
        }
        validate_folders(&input.folders)?;
        let mut tx = begin_productivity(self).await?;
        product_project(&mut tx, project).await?;
        let current = folders_in_tx(&mut tx, project, is_personal).await?;
        if current.revision != input.expected_revision {
            if current.revision == input.expected_revision.saturating_add(1)
                && current.folders == input.folders
            {
                return Ok(current);
            }
            return Err(Error::Conflict);
        }
        let ids: BTreeSet<_> = input.folders.iter().map(|f| &f.id).collect();
        if corpus(&mut tx, project, is_personal)
            .await?
            .iter()
            .any(|d| d.folder_id.as_ref().is_some_and(|f| !ids.contains(f)))
        {
            return Err(Error::Validation("knowledge_folder_not_empty".into()));
        }
        let result = KnowledgeFolders {
            revision: current.revision.checked_add(1).ok_or(Error::Conflict)?,
            folders: input.folders,
        };
        sqlx::query("INSERT INTO native_knowledge_folders(scope,payload) VALUES($1,$2) ON CONFLICT(scope) DO UPDATE SET payload=excluded.payload").bind(scope(project,is_personal)).bind(json_string(&result)?).execute(&mut *tx).await.map_err(storage_error)?;
        for graph in graphs_in_tx(&mut tx, project, is_personal).await? {
            if !result.folders.iter().any(|f| f.id == graph.folder_id) {
                sqlx::query("DELETE FROM native_knowledge_graphs WHERE scope=$1 AND folder_id=$2")
                    .bind(scope(project, is_personal))
                    .bind(graph.folder_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            }
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn build_knowledge_graph(
        &self,
        project: &str,
        is_personal: bool,
        input: BuildKnowledgeGraph,
    ) -> Result<KnowledgeTopicGraph> {
        let mut tx = begin_productivity(self).await?;
        product_project(&mut tx, project).await?;
        let graphs = graphs_in_tx(&mut tx, project, is_personal).await?;
        if graphs.len() >= 32
            && !graphs.iter().any(|g| {
                g.folder_id == input.folder_id && g.include_descendants == input.include_descendants
            })
        {
            return Err(Error::Validation("knowledge_graph_count_limit".into()));
        }
        let folders = folders_in_tx(&mut tx, project, is_personal).await?;
        let docs = corpus(&mut tx, project, is_personal).await?;
        let graph = make_graph(&folders.folders, &docs, &input)?;
        sqlx::query("INSERT INTO native_knowledge_graphs(scope,folder_id,descendants,payload) VALUES($1,$2,$3,$4) ON CONFLICT(scope,folder_id,descendants) DO UPDATE SET payload=excluded.payload").bind(scope(project,is_personal)).bind(&input.folder_id).bind(i64::from(input.include_descendants)).bind(json_string(&graph)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(graph)
    }
    pub async fn knowledge_graphs(
        &self,
        project: &str,
        is_personal: bool,
    ) -> Result<Vec<KnowledgeTopicGraph>> {
        let mut tx = begin_productivity(self).await?;
        product_project(&mut tx, project).await?;
        let folders = folders_in_tx(&mut tx, project, is_personal).await?;
        let docs = corpus(&mut tx, project, is_personal).await?;
        let mut graphs = graphs_in_tx(&mut tx, project, is_personal).await?;
        for graph in &mut graphs {
            graph.stale = fingerprint(
                &folders.folders,
                &docs,
                &graph.folder_id,
                graph.include_descendants,
            )
            .map_or(true, |f| f != graph.fingerprint);
        }
        Ok(graphs)
    }
    pub async fn search_knowledge_graph(
        &self,
        project: &str,
        is_personal: bool,
        input: SearchKnowledgeGraph,
    ) -> Result<KnowledgeGraphResults> {
        if input.query.trim().is_empty() || input.query.len() > 500 {
            return Err(Error::Validation("invalid_knowledge_search".into()));
        }
        let mut tx = begin_productivity(self).await?;
        product_project(&mut tx, project).await?;
        let folders = folders_in_tx(&mut tx, project, is_personal).await?;
        let docs = corpus(&mut tx, project, is_personal).await?;
        let graph = graphs_in_tx(&mut tx, project, is_personal)
            .await?
            .into_iter()
            .find(|g| {
                g.folder_id == input.folder_id && g.include_descendants == input.include_descendants
            })
            .ok_or(Error::NotFound)?;
        if fingerprint(
            &folders.folders,
            &docs,
            &graph.folder_id,
            graph.include_descendants,
        )? != graph.fingerprint
        {
            return Err(Error::Validation("knowledge_graph_stale".into()));
        }
        Ok(KnowledgeGraphResults {
            hits: retrieve(&graph, &docs, &input.query),
            fingerprint: graph.fingerprint,
        })
    }
}

/// Called in the run-admission transaction. Only current graphs and entries that
/// already pass the run's existing scope/access rules may influence retrieval.
pub(crate) async fn automatic_hits(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    spec: &RunSpec,
) -> Result<BTreeMap<String, (usize, String)>> {
    let gate = crate::knowledge_access::local_gate(tx, &spec.project_id).await?;
    let mut hits = BTreeMap::<String, (usize, String)>::new();
    for is_personal in [false, true] {
        if !(if is_personal {
            gate.personal
        } else {
            gate.project
        }) {
            continue;
        }
        let graphs = graphs_in_tx(tx, &spec.project_id, is_personal).await?;
        if graphs.is_empty() {
            continue;
        }
        let folders = folders_in_tx(tx, &spec.project_id, is_personal).await?;
        let docs = corpus(tx, &spec.project_id, is_personal).await?;
        let eligible: Vec<_> = docs
            .iter()
            .filter(|d| crate::memory_context::applies(d, spec))
            .cloned()
            .collect();
        for graph in graphs {
            if fingerprint(
                &folders.folders,
                &docs,
                &graph.folder_id,
                graph.include_descendants,
            )
            .map_or(true, |f| f != graph.fingerprint)
            {
                continue;
            }
            for hit in retrieve(&graph, &eligible, &spec.prompt) {
                let value = (
                    hit.score,
                    format!(
                        "topic graph {}, fingerprint {}, via {}",
                        graph.folder_id, graph.fingerprint, hit.via
                    ),
                );
                if hits
                    .get(&hit.document.id)
                    .is_none_or(|(score, _)| *score < hit.score)
                {
                    hits.insert(hit.document.id, value);
                }
            }
        }
    }
    Ok(hits)
}
