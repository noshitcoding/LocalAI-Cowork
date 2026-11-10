//! Read-only scope status. Execution and accounting remain owned by Progress.
use cowork_workflow_contracts::{WorkflowNodeKind, WorkflowNodeStatus, WorkflowNodeView};

/// Keep entry groups in the original manifest order. Generated IDs are hashes;
/// their lexical order is unrelated to the input list or execution admission.
pub(super) fn iteration_order(
    nodes: &mut [WorkflowNodeView],
    manifests: &std::collections::BTreeMap<
        String,
        cowork_domain::workflows::WorkflowIterationManifest,
    >,
) {
    for manifest in manifests.values() {
        let positions: std::collections::BTreeMap<_, _> = manifest
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| (item.id.as_str(), index))
            .collect();
        let slots: Vec<_> = nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| positions.contains_key(node.id.as_str()))
            .map(|(index, _)| index)
            .collect();
        let mut entries: Vec<_> = slots.iter().map(|&index| nodes[index].clone()).collect();
        entries.sort_by_key(|node| positions[node.id.as_str()]);
        for (index, entry) in slots.into_iter().zip(entries) {
            nodes[index] = entry;
        }
    }
}

pub(super) fn composition_status(nodes: &mut [WorkflowNodeView]) {
    let snapshot = nodes.to_vec();
    for scope in nodes
        .iter_mut()
        .filter(|node| node.kind == Some(WorkflowNodeKind::Subflow))
    {
        if !matches!(
            scope.state,
            WorkflowNodeStatus::Pending | WorkflowNodeStatus::Ready
        ) {
            continue;
        }
        let members: Vec<_> = snapshot
            .iter()
            .filter(|node| {
                let mut parent = node.scope_id.as_deref();
                for _ in 0..snapshot.len() {
                    match parent {
                        Some(id) if id == scope.id => return true,
                        Some(id) => {
                            parent = snapshot
                                .iter()
                                .find(|node| node.id == id)
                                .and_then(|node| node.scope_id.as_deref())
                        }
                        None => return false,
                    }
                }
                false
            })
            .collect();
        let has = |state| members.iter().any(|node| node.state == state);
        scope.state = if has(WorkflowNodeStatus::WaitingApproval) {
            WorkflowNodeStatus::WaitingApproval
        } else if has(WorkflowNodeStatus::WaitingInput) {
            WorkflowNodeStatus::WaitingInput
        } else if has(WorkflowNodeStatus::Running) || has(WorkflowNodeStatus::Ready) {
            WorkflowNodeStatus::Running
        } else if has(WorkflowNodeStatus::WaitingTimer) {
            WorkflowNodeStatus::WaitingTimer
        } else if has(WorkflowNodeStatus::Interrupted) {
            WorkflowNodeStatus::Interrupted
        } else {
            scope.state.clone()
        };
        if scope.state == WorkflowNodeStatus::WaitingTimer {
            scope.wake_at = members.iter().filter_map(|node| node.wake_at).min();
        }
    }
}
