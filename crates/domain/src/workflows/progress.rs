//! Deterministic workflow reducer. Checkpoints and decisions are data, not grants.
//! Callers must compare-and-swap and durably store the returned revision before IO.
use super::*;
mod composition;
mod iteration;
mod timers;
use crate::hash;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowNodeState {
    Pending,
    Skipped {
        reason: String,
    },
    Running {
        token: String,
    },
    WaitingChildren,
    WaitingTimer {
        wake_at: i64,
        output: String,
        sha256: String,
    },
    Completed {
        output: String,
        sha256: String,
    },
    Failed {
        code: String,
    },
    Blocked,
    Interrupted,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowState {
    Ready,
    Running,
    Completed,
    Failed,
    Interrupted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNodeProgress {
    pub plan: WorkflowNodePlan,
    pub state: WorkflowNodeState,
    pub started_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowClaim {
    pub node_id: String,
    pub token: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowOperation {
    /// Pure deterministic proposal preparation; no provider send or token usage.
    Preparation {
        request_sha256: String,
    },
    Control {
        request_sha256: String,
    },
    Model {
        request_sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reserved_tokens: Option<u64>,
    },
    Tool {
        tool_id: String,
        arguments_sha256: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowOutcome {
    Measured {
        input_tokens: u64,
        output_tokens: u64,
    },
    Unmeasured {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    Succeeded,
    Failed,
    Uncertain,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAttempt {
    pub id: String,
    pub step: u32,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub claim: WorkflowClaim,
    pub operation: WorkflowOperation,
    pub reserved_tokens: u64,
    pub outcome: Option<WorkflowOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_sha256: Option<String>,
}
#[derive(Clone, Debug)]
pub struct WorkflowResponseEvidence {
    pub outcome: WorkflowOutcome,
    pub sha256: String,
}
/// The nested model request remains bound to its original pending tool.
#[derive(Clone, Debug)]
pub struct WorkflowSamplingRequest<'a> {
    pub parent_tool_id: &'a str,
    pub request_sha256: &'a str,
    pub reserved_tokens: Option<u64>,
}
/// Confirmed tool result fields; these are reducer arguments, not stored authority.
#[derive(Clone, Debug)]
pub struct WorkflowDelegationEvidence<'a> {
    pub arguments_sha256: &'a str,
    pub response_sha256: &'a str,
    pub requests: Vec<WorkflowDelegation>,
}
#[derive(Clone, Debug)]
pub struct WorkflowExternalToolEvidence<'a> {
    pub response_sha256: &'a str,
    pub succeeded: bool,
    pub evidence_sha256: &'a str,
}
impl WorkflowAttempt {
    pub fn charged_tokens(&self) -> u64 {
        match &self.outcome {
            Some(WorkflowOutcome::Measured {
                input_tokens,
                output_tokens,
            }) => input_tokens.saturating_add(*output_tokens),
            Some(WorkflowOutcome::Unmeasured {
                input_tokens,
                output_tokens,
            }) => self.reserved_tokens.max(
                input_tokens
                    .unwrap_or(0)
                    .saturating_add(output_tokens.unwrap_or(0)),
            ),
            _ => self.reserved_tokens,
        }
    }
}
/// Not Clone or Deserialize: replaying an admission never returns a new permit.
/// This value alone is not durable admission; persist its checkpoint first.
#[derive(Debug)]
pub struct WorkflowPermit {
    id: String,
    claim: WorkflowClaim,
    operation: WorkflowOperation,
}
impl WorkflowPermit {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn into_dispatch(self) -> (String, WorkflowClaim, WorkflowOperation) {
        (self.id, self.claim, self.operation)
    }
}
#[derive(Debug)]
pub enum WorkflowAdmission {
    Dispatch(WorkflowPermit),
    Existing(WorkflowAttempt),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDelegation {
    pub role_id: String,
    pub goal: String,
    pub rights: WorkflowRights,
    pub budgets: Budgets,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DelegationReceipt {
    claim: WorkflowClaim,
    request_sha256: String,
    children: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum Decision {
    TimersAdvanced {
        nodes: Vec<String>,
    },
    TimersClosed {
        reason: String,
    },
    Native {
        id: String,
        claim: WorkflowClaim,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Box<serde_json::Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clock_ms: Option<i64>,
    },
    HumanPrepared {
        id: String,
        generation: u64,
        cursors: Vec<WorkflowHumanCursor>,
    },
    HumanActivated {
        id: String,
        generation: u64,
        lease_sha256: String,
        claims: Vec<WorkflowClaim>,
    },
    HumanBlocked {
        id: String,
        generation: u64,
        reason: String,
    },
    HumanAnswer {
        claim: WorkflowClaim,
        owner: WorkflowClaim,
        id: String,
        evidence: WorkflowInputAnswerEvidence,
    },
    HumanConsumedAdmit {
        claim: WorkflowClaim,
        id: String,
        operation: WorkflowOperation,
    },
    Claim {
        claim: WorkflowClaim,
    },
    Admit {
        id: String,
        claim: WorkflowClaim,
        operation: WorkflowOperation,
    },
    ToolSamplingAdmitted {
        id: String,
        claim: WorkflowClaim,
        parent_tool_id: String,
        request_sha256: String,
        reserved_tokens: Option<u64>,
    },
    Settle {
        id: String,
        claim: WorkflowClaim,
        outcome: WorkflowOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_sha256: Option<String>,
    },
    Prepared {
        id: String,
        claim: WorkflowClaim,
        request_sha256: String,
        response_sha256: String,
    },
    Complete {
        claim: WorkflowClaim,
        output: String,
    },
    Fail {
        claim: WorkflowClaim,
        code: String,
    },
    Delegate {
        id: String,
        claim: WorkflowClaim,
        requests: Vec<WorkflowDelegation>,
    },
    ToolResult {
        id: String,
        claim: WorkflowClaim,
        tool_id: String,
        arguments_sha256: String,
        response_sha256: String,
        succeeded: bool,
    },
    DelegatedToolResult {
        id: String,
        claim: WorkflowClaim,
        arguments_sha256: String,
        response_sha256: String,
        requests: Vec<WorkflowDelegation>,
    },
    InputOpened {
        id: String,
        claim: WorkflowClaim,
        arguments_sha256: String,
    },
    InputAnswered {
        id: String,
        claim: WorkflowClaim,
        evidence: WorkflowInputAnswerEvidence,
    },
    Recover,
    ResumeSettled {
        completed: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cursors: Vec<WorkflowSettledCursor>,
    },
    ExternalReconciled {
        id: String,
        response_sha256: String,
        succeeded: bool,
        evidence_sha256: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionRecord {
    at: i64,
    decision: Decision,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputAnswerEvidence {
    pub receipt_sha256: String,
    pub confirmed_sequence: i64,
    pub answer_key_sha256: String,
}
/// Replay-validated human wait; it never provides an external dispatch permit.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputCheckpoint {
    pub claim: WorkflowClaim,
    pub arguments_sha256: String,
    pub receipt_sha256: Option<String>,
    pub confirmed_sequence: Option<i64>,
    pub answer_key_sha256: Option<String>,
    pub closed_reason: Option<String>,
}
/// Exact immutable handover from a confirmed model turn. These are evidence,
/// never a provider dispatch permit. Model and human owners are distinct.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowHumanCursor {
    pub node_id: String,
    pub model_claim: WorkflowClaim,
    pub input_claim: WorkflowClaim,
    pub model_turn_id: String,
    pub request_id: String,
    pub fingerprint: String,
    pub context_sha256: String,
    pub response_sha256: String,
    pub prefix_sha256: String,
    pub next_call_ordinal: u32,
    pub expires_at: i64,
    pub scope_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowHumanContinuation {
    pub id: String,
    pub generation: u64,
    pub cursors: Vec<WorkflowHumanCursor>,
    pub execution_claims: Vec<WorkflowClaim>,
    pub lease_sha256: Option<String>,
    pub blocked_reason: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub consumed: BTreeMap<String, String>,
}
/// Confirmed model answer with remaining tools that have never been admitted.
/// It bridges to a fresh claim without changing the frozen scope or old attempt.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSettledCursor {
    pub node_id: String,
    pub model_turn_id: String,
    pub model_claim: WorkflowClaim,
    pub context_sha256: String,
    pub response_sha256: String,
    pub prefix_sha256: String,
    pub next_call_ordinal: u32,
    pub execution_claim: Option<WorkflowClaim>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowProgress {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    iterations: BTreeMap<String, WorkflowIterationManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control_input: Option<Box<serde_json::Value>>,
    format_version: u32,
    plan_sha256: String,
    revision: u64,
    started_at: Option<i64>,
    last_observed_at: Option<i64>,
    nodes: BTreeMap<String, WorkflowNodeProgress>,
    attempts: BTreeMap<String, WorkflowAttempt>,
    delegations: BTreeMap<String, DelegationReceipt>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    inputs: BTreeMap<String, WorkflowInputCheckpoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    human_continuations: Vec<WorkflowHumanContinuation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    settled_cursors: Vec<WorkflowSettledCursor>,
    history: Vec<DecisionRecord>,
}
impl WorkflowProgress {
    pub fn control_input(&self) -> Option<&serde_json::Value> {
        self.control_input.as_deref()
    }
    fn skip_reason(
        &self,
        plan: &WorkflowExecutionGraph<'_>,
        node: &str,
    ) -> Result<Option<&'static str>> {
        if plan.iterations.get(node).is_some_and(|iteration| {
            matches!(
                self.nodes[&iteration.capture_node].state,
                WorkflowNodeState::Skipped { .. }
            )
        }) {
            return Ok(Some("dependency_skipped"));
        }
        if let Some(gate) = plan.definition.gates.get(node) {
            match &self.nodes.get(&gate.step_id).ok_or(Error::Forbidden)?.state {
                WorkflowNodeState::Skipped { .. } => return Ok(Some("condition_skipped")),
                WorkflowNodeState::Completed { output, .. } => {
                    let value = plan.definition.decode_step_output(&gate.step_id, output)?;
                    if value.as_bool().ok_or(Error::Forbidden)? != gate.when {
                        return Ok(Some("condition_not_met"));
                    }
                }
                _ => return Err(Error::Conflict),
            }
        }
        let join = matches!(
            plan.definition.controls.get(node).map(|s| &s.operation),
            Some(
                WorkflowControlOperation::Select { .. } | WorkflowControlOperation::Collect { .. }
            )
        );
        if !join
            && self.nodes[node]
                .plan
                .dependencies
                .iter()
                .any(|id| matches!(self.nodes[id].state, WorkflowNodeState::Skipped { .. }))
        {
            return Ok(Some("dependency_skipped"));
        }
        Ok(None)
    }
    pub fn requires_native(&self, plan: &WorkflowPlan, node: &str) -> Result<bool> {
        // A classification is not an admission. Static definitions need no
        // reconstruction; every state transition still replay-validates below.
        if plan.iteration_program.is_some() {
            self.validate(plan)?;
        }
        self.requires_native_graph(&self.raw_execution_graph(plan)?, node)
    }
    /// Classification against an already reconstructed view, never IO authority.
    /// The caller may reuse one validated view while checking a stored snapshot.
    pub fn requires_native_in(
        &self,
        graph: &WorkflowExecutionGraph<'_>,
        node: &str,
    ) -> Result<bool> {
        if self.plan_sha256 != graph.sha256 {
            return Err(Error::Forbidden);
        }
        self.requires_native_graph(graph, node)
    }
    fn requires_native_graph(&self, plan: &WorkflowExecutionGraph<'_>, node: &str) -> Result<bool> {
        if !self.nodes.contains_key(node) {
            return Err(Error::NotFound);
        }
        Ok((plan.definition.controls.contains_key(node)
            && plan.definition.review_form(node).is_none())
            || self.skip_reason(plan, node)?.is_some())
    }
    pub fn complete_control(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
        input: Option<&serde_json::Value>,
        at: i64,
    ) -> Result<()> {
        plan.definition.validate_data_input(input)?;
        if self
            .control_input
            .as_deref()
            .is_some_and(|saved| Some(saved) != input)
        {
            return Err(Error::Forbidden);
        }
        self.edit(
            plan,
            expected,
            at,
            Decision::Native {
                id: id.into(),
                claim: claim.clone(),
                input: if self.control_input.is_none() {
                    input.cloned().map(Box::new)
                } else {
                    None
                },
                clock_ms: None,
            },
        )
    }
    pub fn settled_cursors(&self) -> &[WorkflowSettledCursor] {
        &self.settled_cursors
    }
    pub fn settled_authority(&self, turn: &str, model: &WorkflowClaim) -> Result<&WorkflowClaim> {
        let cursor = self
            .settled_cursors
            .iter()
            .rev()
            .find(|c| c.model_turn_id == turn && c.model_claim == *model)
            .ok_or(Error::StaleLease)?;
        let claim = cursor.execution_claim.as_ref().ok_or(Error::StaleLease)?;
        self.active(claim)?;
        if self
            .attempts
            .values()
            .any(|a| a.claim == *claim && matches!(a.operation, WorkflowOperation::Model { .. }))
        {
            return Err(Error::StaleLease);
        }
        Ok(claim)
    }
    pub fn human_continuations(&self) -> &[WorkflowHumanContinuation] {
        &self.human_continuations
    }
    pub fn prepare_human(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        id: &str,
        cursors: Vec<WorkflowHumanCursor>,
        at: i64,
    ) -> Result<u64> {
        let generation = self.human_continuations.last().map_or(Ok(1), |c| {
            c.generation.checked_add(1).ok_or(Error::BudgetExceeded)
        })?;
        self.edit(
            plan,
            expected,
            at,
            Decision::HumanPrepared {
                id: id.into(),
                generation,
                cursors,
            },
        )?;
        Ok(generation)
    }
    pub fn activate_human(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        id: &str,
        generation: u64,
        lease: &str,
        at: i64,
    ) -> Result<()> {
        let current = self.human_continuations.last().ok_or(Error::Forbidden)?;
        let claims = current
            .cursors
            .iter()
            .map(|c| WorkflowClaim {
                node_id: c.node_id.clone(),
                token: uuid::Uuid::new_v4().to_string(),
            })
            .collect();
        self.edit(
            plan,
            expected,
            at,
            Decision::HumanActivated {
                id: id.into(),
                generation,
                lease_sha256: hash(lease.as_bytes()),
                claims,
            },
        )
    }
    pub fn block_human(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        id: &str,
        generation: u64,
        reason: &str,
        at: i64,
    ) -> Result<()> {
        self.edit(
            plan,
            expected,
            at,
            Decision::HumanBlocked {
                id: id.into(),
                generation,
                reason: reason.into(),
            },
        )
    }
    /// Only an exact activated bridge can authorize work on an old confirmed turn.
    pub fn human_authority(&self, turn: &str, model: &WorkflowClaim) -> Result<&WorkflowClaim> {
        let c = self
            .human_continuations
            .iter()
            .rev()
            .find(|c| {
                c.cursors
                    .iter()
                    .any(|q| q.model_turn_id == turn && q.model_claim == *model)
            })
            .ok_or(Error::StaleLease)?;
        if c.blocked_reason.is_some() || c.consumed.contains_key(&model.node_id) {
            return Err(Error::StaleLease);
        }
        let claim = c
            .execution_claims
            .iter()
            .find(|q| q.node_id == model.node_id)
            .ok_or(Error::StaleLease)?;
        self.active(claim)?;
        Ok(claim)
    }
    pub fn receipt_claim_valid(
        &self,
        turn: &str,
        model: &WorkflowClaim,
        execution: &WorkflowClaim,
        ordinal: usize,
    ) -> bool {
        model == execution
            || self.settled_cursors.iter().any(|c| {
                c.model_turn_id == turn
                    && c.model_claim == *model
                    && c.execution_claim.as_ref() == Some(execution)
                    && ordinal >= c.next_call_ordinal as usize
            })
            || self.human_continuations.iter().any(|c| {
                c.blocked_reason.is_none()
                    && c.execution_claims.contains(execution)
                    && c.cursors.iter().any(|q| {
                        q.model_turn_id == turn
                            && q.model_claim == *model
                            && q.node_id == execution.node_id
                            && ordinal >= q.next_call_ordinal as usize
                    })
            })
    }
    #[allow(clippy::too_many_arguments)]
    pub fn answer_human(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        execution: &WorkflowClaim,
        owner: &WorkflowClaim,
        id: &str,
        evidence: &WorkflowInputAnswerEvidence,
        at: i64,
    ) -> Result<()> {
        self.edit(
            plan,
            expected,
            at,
            Decision::HumanAnswer {
                claim: execution.clone(),
                owner: owner.clone(),
                id: id.into(),
                evidence: evidence.clone(),
            },
        )
    }
    pub fn new(plan: &WorkflowPlan) -> Result<Self> {
        plan.validate()?;
        Ok(Self {
            iterations: BTreeMap::new(),
            control_input: None,
            format_version: 1,
            plan_sha256: plan.sha256.clone(),
            revision: 0,
            started_at: None,
            last_observed_at: None,
            nodes: plan
                .nodes
                .iter()
                .map(|(id, node)| {
                    (
                        id.clone(),
                        WorkflowNodeProgress {
                            plan: node.clone(),
                            state: WorkflowNodeState::Pending,
                            started_at: None,
                        },
                    )
                })
                .collect(),
            attempts: BTreeMap::new(),
            delegations: BTreeMap::new(),
            inputs: BTreeMap::new(),
            human_continuations: vec![],
            settled_cursors: vec![],
            history: vec![],
        })
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Replay-validated seconds for conservative closure evidence, never a clock grant.
    pub fn last_observed_at(&self) -> Option<i64> {
        self.last_observed_at
    }
    pub fn nodes(&self) -> &BTreeMap<String, WorkflowNodeProgress> {
        &self.nodes
    }
    pub fn attempts(&self) -> &BTreeMap<String, WorkflowAttempt> {
        &self.attempts
    }
    pub fn input_checkpoints(&self) -> &BTreeMap<String, WorkflowInputCheckpoint> {
        &self.inputs
    }
    /// Reserves the one human-input step, without returning any dispatch permit.
    pub fn open_input(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
        arguments_sha256: &str,
        at: i64,
    ) -> Result<()> {
        self.validate(plan)?;
        if let Some(existing) = self.inputs.get(id) {
            return if existing.claim == *claim && existing.arguments_sha256 == arguments_sha256 {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        self.edit(
            plan,
            expected,
            at,
            Decision::InputOpened {
                id: id.into(),
                claim: claim.clone(),
                arguments_sha256: arguments_sha256.into(),
            },
        )
    }
    pub fn answer_input(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
        evidence: &WorkflowInputAnswerEvidence,
        at: i64,
    ) -> Result<()> {
        self.validate(plan)?;
        if let Some(existing) = self.inputs.get(id)
            && existing.claim == *claim
            && existing.receipt_sha256.as_deref() == Some(evidence.receipt_sha256.as_str())
            && existing.confirmed_sequence == Some(evidence.confirmed_sequence)
            && existing.answer_key_sha256.as_deref() == Some(evidence.answer_key_sha256.as_str())
        {
            return Ok(());
        }
        self.edit(
            plan,
            expected,
            at,
            Decision::InputAnswered {
                id: id.into(),
                claim: claim.clone(),
                evidence: evidence.clone(),
            },
        )
    }
    pub fn charged_tokens(&self) -> u64 {
        self.attempts
            .values()
            .map(WorkflowAttempt::charged_tokens)
            .fold(0, u64::saturating_add)
    }
    pub fn state(&self) -> WorkflowState {
        if self
            .nodes
            .values()
            .any(|n| n.state == WorkflowNodeState::Interrupted)
        {
            return WorkflowState::Interrupted;
        }
        if self.nodes.values().all(|n| {
            matches!(
                n.state,
                WorkflowNodeState::Completed { .. } | WorkflowNodeState::Skipped { .. }
            )
        }) {
            return WorkflowState::Completed;
        }
        if self.nodes.values().all(|n| terminal_node(&n.state)) {
            return WorkflowState::Failed;
        }
        if self.started_at.is_some() {
            WorkflowState::Running
        } else {
            WorkflowState::Ready
        }
    }
    /// Replays every decision, then compares the reconstructed checkpoint. Hashes
    /// detect corruption but do not replace trusted storage or policy checks.
    pub fn validate(&self, plan: &WorkflowPlan) -> Result<()> {
        if self.history.len() > 16_384
            || self.nodes.len()
                > if plan.iteration_program.is_some() {
                    4096
                } else {
                    128
                }
            || self.attempts.len() > 4096
            || !matches!(self.format_version, 1..=11)
        {
            return invalid("Workflow checkpoint exceeds supported limits");
        }
        let mut replay = Self::new(plan)?;
        let mut graph = replay.raw_execution_graph(plan)?;
        for record in &self.history {
            let count = replay.iterations.len();
            replay.reduce(&graph, record)?;
            if replay.iterations.len() != count {
                graph = replay.raw_execution_graph(plan)?;
            }
        }
        if canonical_hash(&replay) != canonical_hash(self) {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
    fn edit(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        at: i64,
        decision: Decision,
    ) -> Result<()> {
        if expected != self.revision {
            return Err(Error::Conflict);
        }
        self.validate(plan)?;
        let mut updated = self.clone();
        let graph = updated.raw_execution_graph(plan)?;
        updated.reduce(&graph, &DecisionRecord { at, decision })?;
        *self = updated;
        Ok(())
    }
    pub fn ready_nodes(&self, plan: &WorkflowPlan) -> Result<Vec<String>> {
        self.validate(plan)?;
        let graph = self.raw_execution_graph(plan)?;
        if matches!(
            self.state(),
            WorkflowState::Interrupted | WorkflowState::Completed | WorkflowState::Failed
        ) {
            return Ok(vec![]);
        }
        Ok(self
            .nodes
            .iter()
            .filter(|(id, n)| {
                self.ready(n)
                    && self.composition_capacity(&graph, id)
                    && !self.composition_stopped(&graph, id)
                    && self.iteration_capacity(&graph, id)
            })
            .map(|(id, _)| id.clone())
            .collect())
    }
    pub fn claim(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        node_id: &str,
        at: i64,
    ) -> Result<WorkflowClaim> {
        let claim = WorkflowClaim {
            node_id: node_id.into(),
            token: uuid::Uuid::new_v4().to_string(),
        };
        self.edit(
            plan,
            expected,
            at,
            Decision::Claim {
                claim: claim.clone(),
            },
        )?;
        Ok(claim)
    }
    pub fn admit(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
        operation: WorkflowOperation,
        at: i64,
    ) -> Result<WorkflowAdmission> {
        self.validate(plan)?;
        if let Some(existing) = self.attempts.get(id) {
            if existing.claim != *claim || existing.operation != operation {
                return Err(Error::Conflict);
            }
            return Ok(WorkflowAdmission::Existing(existing.clone()));
        }
        let continuation = self.human_continuations.iter().rev().any(|c| {
            c.execution_claims.contains(claim) && !c.consumed.contains_key(&claim.node_id)
        });
        self.edit(
            plan,
            expected,
            at,
            if continuation && matches!(operation, WorkflowOperation::Model { .. }) {
                Decision::HumanConsumedAdmit {
                    id: id.into(),
                    claim: claim.clone(),
                    operation: operation.clone(),
                }
            } else {
                Decision::Admit {
                    id: id.into(),
                    claim: claim.clone(),
                    operation: operation.clone(),
                }
            },
        )?;
        Ok(WorkflowAdmission::Dispatch(WorkflowPermit {
            id: id.into(),
            claim: claim.clone(),
            operation,
        }))
    }
    /// Nested model IO is admitted only under the exact pending MCP tool claim.
    /// The ordinary model admission keeps its no-pending invariant.
    pub fn admit_tool_sampling(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
        request: WorkflowSamplingRequest<'_>,
        at: i64,
    ) -> Result<WorkflowAdmission> {
        let WorkflowSamplingRequest {
            parent_tool_id,
            request_sha256,
            reserved_tokens,
        } = request;
        self.validate(plan)?;
        let operation = WorkflowOperation::Model {
            request_sha256: request_sha256.into(),
            reserved_tokens,
        };
        if let Some(old) = self.attempts.get(id) {
            let original_parent = self.history.iter().find_map(|r| match &r.decision {
                Decision::ToolSamplingAdmitted {
                    id: original_id,
                    parent_tool_id,
                    ..
                } if original_id == id => Some(parent_tool_id.as_str()),
                _ => None,
            });
            if old.claim != *claim
                || old.operation != operation
                || original_parent != Some(parent_tool_id)
            {
                return Err(Error::Conflict);
            }
            return Ok(WorkflowAdmission::Existing(old.clone()));
        }
        self.edit(
            plan,
            expected,
            at,
            Decision::ToolSamplingAdmitted {
                id: id.into(),
                claim: claim.clone(),
                parent_tool_id: parent_tool_id.into(),
                request_sha256: request_sha256.into(),
                reserved_tokens,
            },
        )?;
        Ok(WorkflowAdmission::Dispatch(WorkflowPermit {
            id: id.into(),
            claim: claim.clone(),
            operation,
        }))
    }
    pub fn settle(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
        outcome: WorkflowOutcome,
        at: i64,
    ) -> Result<()> {
        self.validate(plan)?;
        if let Some(existing) = self.attempts.get(id)
            && existing.outcome.as_ref() == Some(&outcome)
            && existing.claim == *claim
            && existing.response_sha256.is_none()
        {
            return Ok(());
        }
        self.edit(
            plan,
            expected,
            at,
            Decision::Settle {
                id: id.into(),
                claim: claim.clone(),
                outcome,
                response_sha256: None,
            },
        )
    }
    /// Bind the complete model answer to the replay-validated accounting receipt.
    pub fn settle_response(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
        evidence: WorkflowResponseEvidence,
        at: i64,
    ) -> Result<()> {
        self.validate(plan)?;
        if !self.attempts.get(id).is_some_and(|a| {
            matches!(
                a.operation,
                WorkflowOperation::Model { .. } | WorkflowOperation::Preparation { .. }
            )
        }) {
            return Err(Error::Forbidden);
        }
        if let Some(existing) = self.attempts.get(id)
            && existing.outcome.as_ref() == Some(&evidence.outcome)
            && existing.claim == *claim
            && existing.response_sha256.as_ref() == Some(&evidence.sha256)
        {
            return Ok(());
        }
        self.edit(
            plan,
            expected,
            at,
            Decision::Settle {
                id: id.into(),
                claim: claim.clone(),
                outcome: evidence.outcome,
                response_sha256: Some(evidence.sha256),
            },
        )
    }
    /// Confirm the retained physical response of an already admitted external
    /// tool. This grants no dispatch/retry authority, including for uncertainty.
    #[allow(clippy::too_many_arguments)]
    pub fn settle_tool_response(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
        response_sha256: &str,
        succeeded: bool,
        at: i64,
    ) -> Result<()> {
        self.validate(plan)?;
        let attempt = self.attempts.get(id).ok_or(Error::NotFound)?;
        if !matches!(attempt.operation, WorkflowOperation::Tool { .. }) || attempt.claim != *claim {
            return Err(Error::Forbidden);
        }
        let outcome = if succeeded {
            WorkflowOutcome::Succeeded
        } else {
            WorkflowOutcome::Failed
        };
        if attempt.outcome.as_ref() == Some(&outcome)
            && attempt.response_sha256.as_deref() == Some(response_sha256)
        {
            return Ok(());
        }
        if attempt.outcome.is_some() {
            return Err(Error::UncertainEffect);
        }
        self.edit(
            plan,
            expected,
            at,
            Decision::Settle {
                id: id.into(),
                claim: claim.clone(),
                outcome,
                response_sha256: Some(response_sha256.into()),
            },
        )
    }
    /// Database-only tool admission and its immutable result are one decision.
    /// This never grants permission for external IO or an uncertain side effect.
    #[allow(clippy::too_many_arguments)]
    pub fn record_tool_result(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
        tool_id: &str,
        arguments_sha256: &str,
        response_sha256: &str,
        succeeded: bool,
        at: i64,
    ) -> Result<()> {
        self.validate(plan)?;
        let operation = WorkflowOperation::Tool {
            tool_id: tool_id.into(),
            arguments_sha256: arguments_sha256.into(),
        };
        let outcome = if succeeded {
            WorkflowOutcome::Succeeded
        } else {
            WorkflowOutcome::Failed
        };
        if let Some(existing) = self.attempts.get(id) {
            return if existing.claim == *claim
                && existing.operation == operation
                && existing.outcome.as_ref() == Some(&outcome)
                && existing.response_sha256.as_deref() == Some(response_sha256)
            {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        self.edit(
            plan,
            expected,
            at,
            Decision::ToolResult {
                id: id.into(),
                claim: claim.clone(),
                tool_id: tool_id.into(),
                arguments_sha256: arguments_sha256.into(),
                response_sha256: response_sha256.into(),
                succeeded,
            },
        )
    }
    /// One durable decision freezes a native proposal and its exact response.
    /// No external operation has been authorized or performed by this decision.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_step(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
        request_sha256: &str,
        response_sha256: &str,
        at: i64,
    ) -> Result<()> {
        self.edit(
            plan,
            expected,
            at,
            Decision::Prepared {
                id: id.into(),
                claim: claim.clone(),
                request_sha256: request_sha256.into(),
                response_sha256: response_sha256.into(),
            },
        )
    }
    pub fn complete(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        output: &str,
        at: i64,
    ) -> Result<()> {
        self.edit(
            plan,
            expected,
            at,
            Decision::Complete {
                claim: claim.clone(),
                output: output.into(),
            },
        )
    }
    pub fn fail(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        code: &str,
        at: i64,
    ) -> Result<()> {
        self.edit(
            plan,
            expected,
            at,
            Decision::Fail {
                claim: claim.clone(),
                code: code.into(),
            },
        )
    }
    pub fn delegate(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
        requests: Vec<WorkflowDelegation>,
        at: i64,
    ) -> Result<Vec<String>> {
        self.validate(plan)?;
        if let Some(existing) = self.delegations.get(id) {
            if existing.claim != *claim || existing.request_sha256 != canonical_hash(&requests) {
                return Err(Error::Conflict);
            }
            return Ok(existing.children.clone());
        }
        self.edit(
            plan,
            expected,
            at,
            Decision::Delegate {
                id: id.into(),
                claim: claim.clone(),
                requests,
            },
        )?;
        Ok(self.delegations[id].children.clone())
    }
    /// A new coordinator may not replay another process's claims/actions. The
    /// unresolved receipts remain visible and require explicit reconciliation.
    pub fn recover(&mut self, plan: &WorkflowPlan, expected: u64, at: i64) -> Result<()> {
        self.edit(plan, expected, at, Decision::Recover)
    }
    pub fn record_delegation_result(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
        evidence: WorkflowDelegationEvidence<'_>,
        at: i64,
    ) -> Result<()> {
        let WorkflowDelegationEvidence {
            arguments_sha256,
            response_sha256,
            requests,
        } = evidence;
        self.edit(
            plan,
            expected,
            at,
            Decision::DelegatedToolResult {
                id: id.into(),
                claim: claim.clone(),
                arguments_sha256: arguments_sha256.into(),
                response_sha256: response_sha256.into(),
                requests,
            },
        )
    }
    /// Only settled histories can become ready again. Attempts are never removed.
    pub fn resume_settled(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        completed: BTreeMap<String, String>,
        at: i64,
    ) -> Result<()> {
        self.edit(
            plan,
            expected,
            at,
            Decision::ResumeSettled {
                completed,
                cursors: vec![],
            },
        )
    }
    pub fn resume_settled_with_tools(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        completed: BTreeMap<String, String>,
        cursors: Vec<WorkflowSettledCursor>,
        at: i64,
    ) -> Result<()> {
        self.edit(
            plan,
            expected,
            at,
            Decision::ResumeSettled { completed, cursors },
        )
    }
    pub fn reconcile_external_tool(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        id: &str,
        evidence: WorkflowExternalToolEvidence<'_>,
        at: i64,
    ) -> Result<()> {
        let WorkflowExternalToolEvidence {
            response_sha256,
            succeeded,
            evidence_sha256,
        } = evidence;
        self.edit(
            plan,
            expected,
            at,
            Decision::ExternalReconciled {
                id: id.into(),
                response_sha256: response_sha256.into(),
                succeeded,
                evidence_sha256: evidence_sha256.into(),
            },
        )
    }
    fn ready(&self, node: &WorkflowNodeProgress) -> bool {
        let dependencies_done = node.plan.dependencies.iter().all(|d| {
            matches!(
                self.nodes[d].state,
                WorkflowNodeState::Completed { .. } | WorkflowNodeState::Skipped { .. }
            )
        });
        match node.state {
            WorkflowNodeState::Pending => dependencies_done,
            WorkflowNodeState::WaitingChildren => {
                dependencies_done
                    && self
                        .nodes
                        .values()
                        .filter(|n| n.plan.parent.as_ref() == Some(&node.plan.id))
                        .all(|n| {
                            matches!(
                                n.state,
                                WorkflowNodeState::Completed { .. }
                                    | WorkflowNodeState::Skipped { .. }
                            )
                        })
            }
            _ => false,
        }
    }
    fn stopped(&self, plan: &WorkflowExecutionGraph<'_>) -> bool {
        plan.definition.failure == WorkflowFailure::Stop
            && self.nodes.values().any(|n| {
                matches!(
                    n.state,
                    WorkflowNodeState::Failed { .. } | WorkflowNodeState::Blocked
                )
            })
    }
    fn active(&self, claim: &WorkflowClaim) -> Result<&WorkflowNodeProgress> {
        let node = self.nodes.get(&claim.node_id).ok_or(Error::NotFound)?;
        if !matches!(&node.state, WorkflowNodeState::Running { token } if token == &claim.token) {
            return Err(Error::StaleLease);
        }
        Ok(node)
    }
    fn no_pending(&self, node_id: &str) -> Result<()> {
        if self.attempts.values().any(|a| {
            a.claim.node_id == node_id
                && (a.outcome.is_none() || a.outcome == Some(WorkflowOutcome::Uncertain))
        }) {
            return Err(Error::UncertainEffect);
        }
        Ok(())
    }
    fn ancestors(&self, id: &str) -> Vec<String> {
        let mut result = vec![id.to_owned()];
        let mut current = self.nodes[id].plan.parent.as_ref();
        while let Some(parent) = current {
            result.push(parent.clone());
            current = self.nodes[parent].plan.parent.as_ref();
        }
        result
    }
    fn descendant(&self, node: &str, ancestor: &str) -> bool {
        self.ancestors(node).iter().any(|n| n == ancestor)
    }
    fn within_time(&self, plan: &WorkflowExecutionGraph<'_>, node_id: &str, at: i64) -> Result<()> {
        if self
            .started_at
            .is_some_and(|s| at - s >= plan.budgets.max_seconds as i64)
        {
            return Err(Error::BudgetExceeded);
        }
        for id in self.ancestors(node_id) {
            let n = &self.nodes[&id];
            if n.started_at
                .is_some_and(|s| at - s >= n.plan.budgets.max_seconds as i64)
            {
                return Err(Error::BudgetExceeded);
            }
        }
        Ok(())
    }
    fn budget_admission(
        &self,
        plan: &WorkflowExecutionGraph<'_>,
        node_id: &str,
        reservation: u64,
    ) -> Result<()> {
        if self.attempts.len() >= plan.budgets.max_steps as usize
            || self.charged_tokens().saturating_add(reservation) > plan.budgets.max_total_tokens
        {
            return Err(Error::BudgetExceeded);
        }
        for id in self.ancestors(node_id) {
            // The return node belongs to the caller. Its own body quota covers
            // body work; the caller and global ledger still charge the return.
            if id == node_id && plan.composition_scopes.contains_key(&id) {
                continue;
            }
            let attempts: Vec<_> = self
                .attempts
                .values()
                .filter(|a| self.descendant(&a.claim.node_id, &id))
                .collect();
            let charged = attempts
                .iter()
                .map(|a| a.charged_tokens())
                .fold(0, u64::saturating_add);
            if attempts.len() >= self.nodes[&id].plan.budgets.max_steps as usize
                || charged.saturating_add(reservation)
                    > self.nodes[&id].plan.budgets.max_total_tokens
            {
                return Err(Error::BudgetExceeded);
            }
        }
        Ok(())
    }
    fn reduce(&mut self, plan: &WorkflowExecutionGraph<'_>, record: &DecisionRecord) -> Result<()> {
        let at = record.at;
        if at < 0 || self.last_observed_at.is_some_and(|last| at < last) {
            return invalid("Workflow clock moved backwards");
        }
        if self.history.len() >= 16_384 {
            return Err(Error::BudgetExceeded);
        }
        self.apply(plan, record)?;
        self.propagate_failure(plan);
        self.revision += 1;
        self.last_observed_at = Some(at);
        self.history.push(record.clone());
        Ok(())
    }
    fn apply(&mut self, plan: &WorkflowExecutionGraph<'_>, record: &DecisionRecord) -> Result<()> {
        let at = record.at;
        match &record.decision {
            Decision::TimersAdvanced { nodes } => self.apply_timers_advanced(plan, nodes, at)?,
            Decision::TimersClosed { reason } => self.apply_timers_closed(reason)?,
            Decision::Native {
                id,
                claim,
                input,
                clock_ms,
            } => {
                if clock_ms.is_some_and(|ms| ms < 0 || ms.div_euclid(1000) != at)
                    || clock_ms.is_some()
                        && !matches!(
                            plan.definition
                                .controls
                                .get(&claim.node_id)
                                .map(|s| &s.operation),
                            Some(WorkflowControlOperation::Wait { .. })
                        )
                {
                    return Err(Error::Forbidden);
                }
                identifier(id)?;
                if self.attempts.contains_key(id)
                    || self.history.iter().any(
                        |r| matches!(&r.decision, Decision::Native { id: old, .. } if old == id),
                    )
                {
                    return Err(Error::Conflict);
                }
                self.active(claim)?;
                self.no_pending(&claim.node_id)?;
                if (self.stopped(plan) || self.composition_stopped(plan, &claim.node_id))
                    || self.state() == WorkflowState::Interrupted
                {
                    return Err(Error::Conflict);
                }
                self.within_time(plan, &claim.node_id, at)?;
                if let Some(input) = input {
                    if self.control_input.is_some() {
                        return Err(Error::Forbidden);
                    }
                    plan.definition.validate_data_input(Some(input))?;
                    self.control_input = Some(input.clone());
                }
                plan.definition
                    .validate_data_input(self.control_input.as_deref())?;
                if let Some(reason) = self.skip_reason(plan, &claim.node_id)? {
                    self.nodes
                        .get_mut(&claim.node_id)
                        .ok_or(Error::NotFound)?
                        .state = WorkflowNodeState::Skipped {
                        reason: reason.into(),
                    };
                } else {
                    if !plan.definition.controls.contains_key(&claim.node_id) {
                        return Err(Error::Forbidden);
                    }
                    let administrative = Self::composition_entry(plan, &claim.node_id);
                    if !administrative {
                        self.budget_admission(plan, &claim.node_id, 0)?;
                    }
                    if self.attempts.contains_key(id) {
                        return Err(Error::Conflict);
                    }
                    let completed = self
                        .nodes
                        .iter()
                        .filter_map(|(id, n)| {
                            if let WorkflowNodeState::Completed { output, .. } = &n.state {
                                Some((id.clone(), output.clone()))
                            } else {
                                None
                            }
                        })
                        .collect();
                    self.capture_iteration(plan, &claim.node_id, &completed)?;
                    let mut value =
                        match self.collect_iteration(plan, &claim.node_id, &completed)? {
                            Some(value) => Some(value),
                            None => plan.definition.evaluate_control_at(
                                &claim.node_id,
                                self.control_input.as_deref(),
                                &completed,
                                Some(at),
                            )?,
                        };
                    let wake_at = if matches!(
                        plan.definition.controls[&claim.node_id].operation,
                        WorkflowControlOperation::Wait { .. }
                    ) {
                        let mut wake_at = value
                            .as_ref()
                            .and_then(|v| v["wake_at"].as_i64())
                            .ok_or(Error::Forbidden)?;
                        if clock_ms.is_some_and(|ms| ms.rem_euclid(1000) != 0) {
                            wake_at = wake_at.checked_add(1).ok_or(Error::Forbidden)?;
                            value = Some(serde_json::json!({"wake_at":wake_at}));
                        }
                        self.within_time(plan, &claim.node_id, wake_at)?;
                        Some(wake_at)
                    } else {
                        None
                    };
                    let output = value
                        .map(|value| serde_json::to_string(&value))
                        .transpose()
                        .map_err(|_| Error::Forbidden)?;
                    let limit = if plan.iterations.contains_key(&claim.node_id)
                        || plan
                            .iterations
                            .values()
                            .any(|iteration| iteration.capture_node == claim.node_id)
                    {
                        256 * 1024
                    } else {
                        65_536
                    };
                    if output.as_ref().is_some_and(|o| o.len() > limit) {
                        return Err(Error::ContextLimit);
                    }
                    let mut request = serde_json::json!({"plan":plan.sha256,"node":claim.node_id,"input":self.control_input,"completed":completed});
                    if let Some(ms) = clock_ms {
                        request["clock_ms"] = serde_json::json!(ms);
                    }
                    let request_sha256 = canonical_hash(&request);
                    if !administrative {
                        self.attempts.insert(
                            id.clone(),
                            WorkflowAttempt {
                                id: id.clone(),
                                step: self.attempts.len() as u32 + 1,
                                started_at: at,
                                finished_at: Some(at),
                                claim: claim.clone(),
                                operation: WorkflowOperation::Control { request_sha256 },
                                reserved_tokens: 0,
                                outcome: Some(WorkflowOutcome::Succeeded),
                                response_sha256: Some(canonical_hash(&output)),
                            },
                        );
                    }
                    self.nodes
                        .get_mut(&claim.node_id)
                        .ok_or(Error::NotFound)?
                        .state = match output {
                        Some(output) => {
                            if let Some(wake_at) = wake_at {
                                WorkflowNodeState::WaitingTimer {
                                    wake_at,
                                    sha256: hash(output.as_bytes()),
                                    output,
                                }
                            } else {
                                WorkflowNodeState::Completed {
                                    sha256: hash(output.as_bytes()),
                                    output,
                                }
                            }
                        }
                        None => WorkflowNodeState::Skipped {
                            reason: "empty_selection".into(),
                        },
                    };
                }
                self.format_version = self.format_version.max(if clock_ms.is_some() {
                    9
                } else if matches!(
                    plan.definition
                        .controls
                        .get(&claim.node_id)
                        .map(|s| &s.operation),
                    Some(WorkflowControlOperation::Wait { .. })
                ) {
                    8
                } else {
                    7
                });
            }

            Decision::HumanPrepared {
                id,
                generation,
                cursors,
            } => {
                identifier(id)?;
                let expected = self
                    .human_continuations
                    .last()
                    .map_or(1, |c| c.generation.saturating_add(1));
                let interrupted: BTreeSet<_> = self
                    .nodes
                    .iter()
                    .filter(|(_, n)| n.state == WorkflowNodeState::Interrupted)
                    .map(|(id, _)| id.clone())
                    .collect();
                let supplied: BTreeSet<_> = cursors.iter().map(|c| c.node_id.clone()).collect();
                if *generation != expected
                    || cursors.is_empty()
                    || supplied.len() != cursors.len()
                    || supplied != interrupted
                    || self.human_continuations.iter().any(|c| c.id == *id)
                    || self.attempts.values().any(|a| {
                        a.outcome == Some(WorkflowOutcome::Uncertain)
                            || (a.outcome.is_none() && !self.inputs.contains_key(&a.id))
                    })
                {
                    return Err(Error::UncertainEffect);
                }
                for c in cursors {
                    for sha in [
                        &c.context_sha256,
                        &c.response_sha256,
                        &c.prefix_sha256,
                        &c.scope_sha256,
                        &c.fingerprint,
                    ] {
                        digest(sha)?;
                    }
                    self.within_time(plan, &c.node_id, at)?;
                    if c.expires_at <= at.saturating_mul(1000)
                        || c.model_claim.node_id != c.node_id
                        || c.input_claim.node_id != c.node_id
                    {
                        return Err(Error::Forbidden);
                    }
                    let model = self
                        .attempts
                        .get(&c.model_turn_id)
                        .ok_or(Error::Forbidden)?;
                    let input = self.inputs.get(&c.request_id).ok_or(Error::Forbidden)?;
                    if model.claim != c.model_claim
                        || model.response_sha256.as_ref() != Some(&c.response_sha256)
                        || input.claim != c.input_claim
                        || input.closed_reason.is_some()
                    {
                        return Err(Error::Forbidden);
                    }
                }
                self.human_continuations.push(WorkflowHumanContinuation {
                    id: id.clone(),
                    generation: *generation,
                    cursors: cursors.clone(),
                    execution_claims: vec![],
                    lease_sha256: None,
                    blocked_reason: None,
                    consumed: BTreeMap::new(),
                });
                self.format_version = self.format_version.max(4);
            }
            Decision::HumanActivated {
                id,
                generation,
                lease_sha256,
                claims,
            } => {
                digest(lease_sha256)?;
                let c = self
                    .human_continuations
                    .last()
                    .ok_or(Error::Forbidden)?
                    .clone();
                if c.id != *id
                    || c.generation != *generation
                    || !c.execution_claims.is_empty()
                    || c.blocked_reason.is_some()
                    || claims.len() != c.cursors.len()
                    || claims.len() > usize::from(plan.max_parallel)
                {
                    return Err(Error::Conflict);
                }
                let mut tokens = BTreeSet::new();
                for (cursor, claim) in c.cursors.iter().zip(claims) {
                    if claim.node_id != cursor.node_id
                        || uuid::Uuid::parse_str(&claim.token).is_err()
                        || !tokens.insert(&claim.token)
                        || self.attempts.values().any(|a| a.claim.token == claim.token)
                        || self.human_continuations.iter().any(|c| {
                            c.execution_claims
                                .iter()
                                .any(|old| old.token == claim.token)
                        })
                    {
                        return Err(Error::Forbidden);
                    }
                    self.within_time(plan, &claim.node_id, at)?;
                    if cursor.expires_at <= at.saturating_mul(1000) {
                        return Err(Error::BudgetExceeded);
                    }
                    let node = self.nodes.get_mut(&claim.node_id).ok_or(Error::Forbidden)?;
                    if node.state != WorkflowNodeState::Interrupted {
                        return Err(Error::Conflict);
                    }
                    node.state = WorkflowNodeState::Running {
                        token: claim.token.clone(),
                    };
                }
                let c = self
                    .human_continuations
                    .last_mut()
                    .ok_or(Error::Forbidden)?;
                c.execution_claims = claims.clone();
                c.lease_sha256 = Some(lease_sha256.clone());
            }
            Decision::HumanBlocked {
                id,
                generation,
                reason,
            } => {
                if !matches!(
                    reason.as_str(),
                    "workflow_revoked"
                        | "canceled"
                        | "budget_exceeded"
                        | "workflow_clock_rollback"
                        | "workflow_clock_unverified"
                ) {
                    return Err(Error::Forbidden);
                }
                let c = self
                    .human_continuations
                    .last_mut()
                    .ok_or(Error::Forbidden)?;
                if c.id != *id
                    || c.generation != *generation
                    || !c.execution_claims.is_empty()
                    || c.blocked_reason.is_some()
                {
                    return Err(Error::Conflict);
                }
                c.blocked_reason = Some(reason.clone());
            }
            Decision::HumanAnswer {
                claim,
                owner,
                id,
                evidence,
            } => {
                self.active(claim)?;
                self.within_time(plan, &claim.node_id, at)?;
                if self.stopped(plan) || self.composition_stopped(plan, &claim.node_id) {
                    return Err(Error::Conflict);
                }
                let bridge = self
                    .human_continuations
                    .iter()
                    .rev()
                    .find(|c| {
                        c.execution_claims.contains(claim)
                            && !c.consumed.contains_key(&claim.node_id)
                    })
                    .ok_or(Error::Forbidden)?;
                if bridge.blocked_reason.is_some()
                    || !bridge
                        .cursors
                        .iter()
                        .any(|q| q.request_id == *id && q.input_claim == *owner)
                {
                    return Err(Error::Forbidden);
                }
                digest(&evidence.receipt_sha256)?;
                digest(&evidence.answer_key_sha256)?;
                if evidence.confirmed_sequence <= 0 {
                    return Err(Error::Forbidden);
                }
                let input = self.inputs.get_mut(id).ok_or(Error::Forbidden)?;
                let attempt = self.attempts.get_mut(id).ok_or(Error::Forbidden)?;
                if input.claim != *owner
                    || attempt.claim != *owner
                    || attempt.outcome.is_some()
                    || input.receipt_sha256.is_some()
                    || input.closed_reason.is_some()
                {
                    return Err(Error::Conflict);
                }
                input.receipt_sha256 = Some(evidence.receipt_sha256.clone());
                input.confirmed_sequence = Some(evidence.confirmed_sequence);
                input.answer_key_sha256 = Some(evidence.answer_key_sha256.clone());
                attempt.outcome = Some(WorkflowOutcome::Succeeded);
                attempt.response_sha256 = Some(evidence.receipt_sha256.clone());
                attempt.finished_at = Some(at);
            }
            Decision::HumanConsumedAdmit {
                claim,
                id,
                operation,
            } => {
                if !matches!(operation, WorkflowOperation::Model { .. }) {
                    return Err(Error::Forbidden);
                }
                let c = self
                    .human_continuations
                    .iter()
                    .rev()
                    .find(|c| {
                        c.execution_claims.contains(claim)
                            && !c.consumed.contains_key(&claim.node_id)
                    })
                    .ok_or(Error::Forbidden)?;
                if c.blocked_reason.is_some() {
                    return Err(Error::Forbidden);
                }
                self.apply(
                    plan,
                    &DecisionRecord {
                        at,
                        decision: Decision::Admit {
                            claim: claim.clone(),
                            id: id.clone(),
                            operation: operation.clone(),
                        },
                    },
                )?;
                let c = self
                    .human_continuations
                    .iter_mut()
                    .rev()
                    .find(|c| {
                        c.execution_claims.contains(claim)
                            && !c.consumed.contains_key(&claim.node_id)
                    })
                    .ok_or(Error::Forbidden)?;
                c.consumed.insert(claim.node_id.clone(), id.clone());
            }
            Decision::Claim { claim } => {
                if uuid::Uuid::parse_str(&claim.token).is_err() || self.history.iter().any(|r| matches!(&r.decision, Decision::Claim { claim: old } if old.token == claim.token)) { return invalid("Invalid or reused workflow claim token"); }
                if matches!(
                    self.state(),
                    WorkflowState::Interrupted | WorkflowState::Completed | WorkflowState::Failed
                ) {
                    return Err(Error::Conflict);
                }
                if self
                    .nodes
                    .values()
                    .filter(|n| matches!(n.state, WorkflowNodeState::Running { .. }))
                    .count()
                    >= usize::from(plan.max_parallel)
                {
                    return Err(Error::WorkspaceBusy);
                }
                let node = self.nodes.get(&claim.node_id).ok_or(Error::NotFound)?;
                if !self.ready(node) || self.composition_stopped(plan, &claim.node_id) {
                    return Err(Error::Conflict);
                }
                if !self.composition_capacity(plan, &claim.node_id)
                    || !self.iteration_capacity(plan, &claim.node_id)
                {
                    return Err(Error::WorkspaceBusy);
                }
                self.within_time(plan, &claim.node_id, at)?;
                self.start_composition(plan, &claim.node_id, at);
                let node = self.nodes.get_mut(&claim.node_id).expect("checked node");
                node.state = WorkflowNodeState::Running {
                    token: claim.token.clone(),
                };
                node.started_at.get_or_insert(at);
                self.started_at.get_or_insert(at);
                if let Some(cursor) = self
                    .settled_cursors
                    .iter_mut()
                    .rev()
                    .find(|c| c.node_id == claim.node_id && c.execution_claim.is_none())
                {
                    cursor.execution_claim = Some(claim.clone());
                }
            }
            Decision::ToolSamplingAdmitted {
                id,
                claim,
                parent_tool_id,
                request_sha256,
                reserved_tokens,
            } => {
                identifier(id)?;
                identifier(parent_tool_id)?;
                digest(request_sha256)?;
                let node = self.active(claim)?;
                if self.state() == WorkflowState::Interrupted
                    || (self.stopped(plan) || self.composition_stopped(plan, &claim.node_id))
                {
                    return Err(Error::UncertainEffect);
                }
                let parent = self.attempts.get(parent_tool_id).ok_or(Error::Forbidden)?;
                if parent.claim != *claim
                    || parent.outcome.is_some()
                    || !matches!(&parent.operation, WorkflowOperation::Tool { tool_id, .. } if tool_id.starts_with("mcp_"))
                    || self.attempts.values().any(|a| {
                        a.claim.node_id == claim.node_id
                            && a.id != *parent_tool_id
                            && (a.outcome.is_none()
                                || a.outcome == Some(WorkflowOutcome::Uncertain))
                    })
                {
                    return Err(Error::UncertainEffect);
                }
                self.within_time(plan, &claim.node_id, at)?;
                let role = &plan.roles[node.plan.role()?];
                let limit = u64::from(role.provider.context_tokens);
                let reservation = match reserved_tokens {
                    Some(n) if role.model_binding.is_some() && *n > 0 && *n <= limit => *n,
                    None => limit,
                    _ => return Err(Error::Forbidden),
                };
                self.budget_admission(plan, &claim.node_id, reservation)?;
                if self.attempts.contains_key(id) {
                    return Err(Error::Conflict);
                }
                self.attempts.insert(
                    id.clone(),
                    WorkflowAttempt {
                        id: id.clone(),
                        step: self.attempts.len() as u32 + 1,
                        started_at: at,
                        finished_at: None,
                        claim: claim.clone(),
                        operation: WorkflowOperation::Model {
                            request_sha256: request_sha256.clone(),
                            reserved_tokens: *reserved_tokens,
                        },
                        reserved_tokens: reservation,
                        outcome: None,
                        response_sha256: None,
                    },
                );
                self.format_version = self.format_version.max(5);
            }
            Decision::Admit {
                id,
                claim,
                operation,
            } => {
                identifier(id)?;
                let node = self.active(claim)?;
                if self.state() == WorkflowState::Interrupted {
                    return Err(Error::UncertainEffect);
                }
                if self.stopped(plan) || self.composition_stopped(plan, &claim.node_id) {
                    return Err(Error::Conflict);
                }
                self.no_pending(&claim.node_id)?;
                self.within_time(plan, &claim.node_id, at)?;
                if self.requires_native_graph(plan, &claim.node_id)? {
                    return Err(Error::Forbidden);
                }
                let reservation = match operation {
                    WorkflowOperation::Control { .. } => return Err(Error::Forbidden),
                    WorkflowOperation::Preparation { request_sha256 } => {
                        digest(request_sha256)?;
                        if !plan
                            .definition
                            .steps
                            .iter()
                            .any(|step| step.id == claim.node_id && step.execution.is_some())
                            && !plan.definition.documents.contains_key(&claim.node_id)
                            && plan.definition.review_form(&claim.node_id).is_none()
                        {
                            return Err(Error::Forbidden);
                        }
                        0
                    }
                    WorkflowOperation::Model {
                        request_sha256,
                        reserved_tokens,
                    } => {
                        digest(request_sha256)?;
                        let role = &plan.roles[node.plan.role()?];
                        let limit = u64::from(role.provider.context_tokens);
                        if let Some(tokens) = reserved_tokens {
                            if role.model_binding.is_none() || *tokens == 0 || *tokens > limit {
                                return Err(Error::Forbidden);
                            }
                            *tokens
                        } else {
                            limit
                        }
                    }
                    WorkflowOperation::Tool {
                        tool_id,
                        arguments_sha256,
                    } => {
                        digest(arguments_sha256)?;
                        if !node.plan.rights.tools.contains(tool_id)
                            || plan
                                .definition
                                .documents
                                .get(&claim.node_id)
                                .is_some_and(|step| step.operation.tool_name() != tool_id)
                            || plan.definition.review_form(&claim.node_id).is_some()
                                && tool_id != "request_input"
                        {
                            return Err(Error::Forbidden);
                        }
                        0
                    }
                };
                self.budget_admission(plan, &claim.node_id, reservation)?;
                if self.attempts.contains_key(id) {
                    return Err(Error::Conflict);
                }
                self.attempts.insert(
                    id.clone(),
                    WorkflowAttempt {
                        id: id.clone(),
                        step: self.attempts.len() as u32 + 1,
                        started_at: at,
                        finished_at: None,
                        claim: claim.clone(),
                        operation: operation.clone(),
                        reserved_tokens: reservation,
                        outcome: None,
                        response_sha256: None,
                    },
                );
            }
            Decision::ToolResult {
                id,
                claim,
                tool_id,
                arguments_sha256,
                response_sha256,
                succeeded,
            } => {
                if tool_id == "request_input" && *succeeded {
                    return Err(Error::Forbidden);
                }
                self.apply(
                    plan,
                    &DecisionRecord {
                        at,
                        decision: Decision::Admit {
                            id: id.clone(),
                            claim: claim.clone(),
                            operation: WorkflowOperation::Tool {
                                tool_id: tool_id.clone(),
                                arguments_sha256: arguments_sha256.clone(),
                            },
                        },
                    },
                )?;
                self.apply(
                    plan,
                    &DecisionRecord {
                        at,
                        decision: Decision::Settle {
                            id: id.clone(),
                            claim: claim.clone(),
                            outcome: if *succeeded {
                                WorkflowOutcome::Succeeded
                            } else {
                                WorkflowOutcome::Failed
                            },
                            response_sha256: Some(response_sha256.clone()),
                        },
                    },
                )?;
                self.format_version = self.format_version.max(2);
            }
            Decision::Prepared {
                id,
                claim,
                request_sha256,
                response_sha256,
            } => {
                self.apply(
                    plan,
                    &DecisionRecord {
                        at,
                        decision: Decision::Admit {
                            id: id.clone(),
                            claim: claim.clone(),
                            operation: WorkflowOperation::Preparation {
                                request_sha256: request_sha256.clone(),
                            },
                        },
                    },
                )?;
                self.apply(
                    plan,
                    &DecisionRecord {
                        at,
                        decision: Decision::Settle {
                            id: id.clone(),
                            claim: claim.clone(),
                            outcome: WorkflowOutcome::Succeeded,
                            response_sha256: Some(response_sha256.clone()),
                        },
                    },
                )?;
                self.format_version = self.format_version.max(6);
            }
            Decision::Settle {
                id,
                claim,
                outcome,
                response_sha256,
            } => {
                if self.inputs.contains_key(id) {
                    return Err(Error::Forbidden);
                }
                let node = self.active(claim)?;
                let output_limit = u64::from(node.plan.budgets.max_output_tokens);
                let attempt = self.attempts.get_mut(id).ok_or(Error::NotFound)?;
                if attempt.claim != *claim || attempt.outcome.is_some() {
                    return Err(Error::Conflict);
                }
                let model = matches!(attempt.operation, WorkflowOperation::Model { .. });
                if let Some(sha) = response_sha256 {
                    digest(sha)?;
                }
                let output = match outcome {
                    WorkflowOutcome::Measured {
                        input_tokens,
                        output_tokens,
                    } => {
                        if !model || input_tokens.checked_add(*output_tokens).is_none() {
                            return invalid("Invalid measured workflow usage");
                        }
                        Some(*output_tokens)
                    }
                    WorkflowOutcome::Unmeasured {
                        input_tokens,
                        output_tokens,
                    } => {
                        if !model
                            || input_tokens
                                .unwrap_or(0)
                                .checked_add(output_tokens.unwrap_or(0))
                                .is_none()
                        {
                            return invalid("Invalid incomplete workflow usage");
                        }
                        *output_tokens
                    }
                    WorkflowOutcome::Succeeded if model => {
                        return invalid("Model usage must be measured or explicitly incomplete");
                    }
                    _ => None,
                };
                attempt.outcome = Some(outcome.clone());
                attempt.response_sha256 = response_sha256.clone();
                attempt.finished_at = Some(at);
                if *outcome == WorkflowOutcome::Uncertain
                    || (model && attempt.charged_tokens() > attempt.reserved_tokens)
                    || output.is_some_and(|n| n > output_limit)
                {
                    self.nodes
                        .get_mut(&claim.node_id)
                        .expect("checked node")
                        .state = WorkflowNodeState::Interrupted;
                }
            }
            Decision::Complete { claim, output } => {
                // Replay already owns the reconstructed graph. Calling the
                // public classifier here recursively replays earlier model
                // completions for every completion in the journal.
                if self.requires_native_graph(plan, &claim.node_id)? {
                    return Err(Error::Forbidden);
                }
                self.active(claim)?;
                self.no_pending(&claim.node_id)?;
                if output.len() > 65_536 || output.contains('\0') {
                    return invalid("Workflow output exceeds checkpoint limit");
                }
                if plan.definition.dataflow.is_some() {
                    plan.definition.decode_step_output(&claim.node_id, output)?;
                }
                self.nodes
                    .get_mut(&claim.node_id)
                    .expect("checked node")
                    .state = WorkflowNodeState::Completed {
                    output: output.clone(),
                    sha256: hash(output.as_bytes()),
                };
            }
            Decision::Fail { claim, code } => {
                self.active(claim)?;
                identifier(code)?;
                // A human wait has no uncertain external effect. Never use this
                // exception for an unconfirmed model or ordinary tool admission.
                if self.attempts.values().any(|a| {
                    a.claim.node_id == claim.node_id
                        && (a.outcome == Some(WorkflowOutcome::Uncertain)
                            || (a.outcome.is_none() && !self.inputs.contains_key(&a.id)))
                }) {
                    return Err(Error::UncertainEffect);
                }
                for (id, input) in &mut self.inputs {
                    if input.claim == *claim
                        && input.receipt_sha256.is_none()
                        && input.closed_reason.is_none()
                    {
                        let attempt = self.attempts.get_mut(id).ok_or(Error::Forbidden)?;
                        if attempt.claim != *claim || attempt.outcome.is_some() {
                            return Err(Error::Forbidden);
                        }
                        attempt.outcome = Some(WorkflowOutcome::Failed);
                        attempt.finished_at = Some(at);
                        input.closed_reason = Some(code.clone());
                    }
                }
                self.no_pending(&claim.node_id)?;
                self.nodes
                    .get_mut(&claim.node_id)
                    .expect("checked node")
                    .state = WorkflowNodeState::Failed { code: code.clone() };
            }
            Decision::Delegate {
                id,
                claim,
                requests,
            } => {
                identifier(id)?;
                let parent = self.active(claim)?.clone();
                if self.stopped(plan) || self.composition_stopped(plan, &claim.node_id) {
                    return Err(Error::Conflict);
                }
                self.no_pending(&claim.node_id)?;
                self.within_time(plan, &claim.node_id, at)?;
                if self.state() == WorkflowState::Interrupted {
                    return Err(Error::UncertainEffect);
                }
                let delegated = self
                    .nodes
                    .values()
                    .filter(|n| n.plan.parent.is_some())
                    .count();
                if requests.is_empty()
                    || requests.len() > 32
                    || self.nodes.len() + requests.len() > 128
                    || delegated + requests.len() > usize::from(plan.max_delegations)
                    || parent.plan.depth >= plan.max_depth
                {
                    return Err(Error::BudgetExceeded);
                }
                if self.delegations.contains_key(id) {
                    return Err(Error::Conflict);
                }
                let mut children = vec![];
                for (i, request) in requests.iter().enumerate() {
                    valid_text(&request.goal, 16_000)?;
                    request.rights.validate()?;
                    validate_budgets(&request.budgets)?;
                    let role = plan.roles.get(&request.role_id).ok_or(Error::NotFound)?;
                    if !plan.roles[parent.plan.role()?]
                        .delegate_to
                        .contains(&role.id)
                    {
                        return Err(Error::Forbidden);
                    }
                    let budgets = minimum_budgets(
                        &minimum_budgets(&parent.plan.budgets, &role.budgets),
                        &request.budgets,
                    );
                    if role.model_binding.is_none()
                        && budgets.max_total_tokens < u64::from(role.provider.context_tokens)
                    {
                        return Err(Error::BudgetConfiguration);
                    }
                    let child_id = format!("delegated-{}", delegated + i + 1);
                    let child = WorkflowNodePlan {
                        id: child_id.clone(),
                        role_id: Some(role.id.clone()),
                        goal: request.goal.clone(),
                        dependencies: BTreeSet::new(),
                        rights: parent
                            .plan
                            .rights
                            .intersect(&role.rights)
                            .intersect(&request.rights),
                        budgets,
                        depth: parent.plan.depth + 1,
                        parent: Some(parent.plan.id.clone()),
                    };
                    self.nodes.insert(
                        child_id.clone(),
                        WorkflowNodeProgress {
                            plan: child,
                            state: WorkflowNodeState::Pending,
                            started_at: None,
                        },
                    );
                    children.push(child_id);
                }
                self.delegations.insert(
                    id.clone(),
                    DelegationReceipt {
                        claim: claim.clone(),
                        request_sha256: canonical_hash(requests),
                        children,
                    },
                );
                self.nodes
                    .get_mut(&claim.node_id)
                    .expect("checked node")
                    .state = WorkflowNodeState::WaitingChildren;
            }
            Decision::InputOpened {
                id,
                claim,
                arguments_sha256,
            } => {
                if self.inputs.contains_key(id) {
                    return Err(Error::Conflict);
                }
                self.apply(
                    plan,
                    &DecisionRecord {
                        at,
                        decision: Decision::Admit {
                            id: id.clone(),
                            claim: claim.clone(),
                            operation: WorkflowOperation::Tool {
                                tool_id: "request_input".into(),
                                arguments_sha256: arguments_sha256.clone(),
                            },
                        },
                    },
                )?;
                self.inputs.insert(
                    id.clone(),
                    WorkflowInputCheckpoint {
                        claim: claim.clone(),
                        arguments_sha256: arguments_sha256.clone(),
                        receipt_sha256: None,
                        confirmed_sequence: None,
                        answer_key_sha256: None,
                        closed_reason: None,
                    },
                );
                self.format_version = self.format_version.max(3);
            }
            Decision::InputAnswered {
                id,
                claim,
                evidence,
            } => {
                digest(&evidence.receipt_sha256)?;
                digest(&evidence.answer_key_sha256)?;
                if evidence.confirmed_sequence <= 0 {
                    return Err(Error::Forbidden);
                }
                if self.stopped(plan) || self.composition_stopped(plan, &claim.node_id) {
                    return Err(Error::Conflict);
                }
                self.within_time(plan, &claim.node_id, at)?;
                let mut input = self.inputs.remove(id).ok_or(Error::NotFound)?;
                if input.claim != *claim || input.receipt_sha256.is_some() {
                    return Err(Error::Conflict);
                }
                self.apply(
                    plan,
                    &DecisionRecord {
                        at,
                        decision: Decision::Settle {
                            id: id.clone(),
                            claim: claim.clone(),
                            outcome: WorkflowOutcome::Succeeded,
                            response_sha256: Some(evidence.receipt_sha256.clone()),
                        },
                    },
                )?;
                input.receipt_sha256 = Some(evidence.receipt_sha256.clone());
                input.confirmed_sequence = Some(evidence.confirmed_sequence);
                input.answer_key_sha256 = Some(evidence.answer_key_sha256.clone());
                self.inputs.insert(id.clone(), input);
                self.format_version = self.format_version.max(3);
            }
            Decision::DelegatedToolResult {
                id,
                claim,
                arguments_sha256,
                response_sha256,
                requests,
            } => {
                self.apply(
                    plan,
                    &DecisionRecord {
                        at,
                        decision: Decision::ToolResult {
                            id: id.clone(),
                            claim: claim.clone(),
                            tool_id: "delegate_agents".into(),
                            arguments_sha256: arguments_sha256.clone(),
                            response_sha256: response_sha256.clone(),
                            succeeded: true,
                        },
                    },
                )?;
                self.apply(
                    plan,
                    &DecisionRecord {
                        at,
                        decision: Decision::Delegate {
                            id: id.clone(),
                            claim: claim.clone(),
                            requests: requests.clone(),
                        },
                    },
                )?;
            }
            Decision::ResumeSettled { completed, cursors } => {
                if self.state() != WorkflowState::Interrupted {
                    // Recovery may land after the last timer was committed but
                    // before parking the parent. Only an entirely quiescent,
                    // settled timer graph may retain its original schedules.
                    if !self.timer_quiescent(plan) || !completed.is_empty() || !cursors.is_empty() {
                        return Err(Error::Conflict);
                    }
                    self.format_version = self.format_version.max(9);
                }
                for node in self.nodes.values() {
                    self.no_pending(&node.plan.id)?;
                }
                if completed.keys().any(|id| {
                    if plan.definition.controls.contains_key(id) {
                        return true;
                    }
                    self.nodes
                        .get(id)
                        .is_none_or(|n| n.state != WorkflowNodeState::Interrupted)
                }) {
                    return Err(Error::Forbidden);
                }
                let mut seen = BTreeSet::new();
                for c in cursors {
                    if !seen.insert(c.node_id.clone())
                        || completed.contains_key(&c.node_id)
                        || c.execution_claim.is_some()
                        || self
                            .nodes
                            .get(&c.node_id)
                            .is_none_or(|n| n.state != WorkflowNodeState::Interrupted)
                        || c.model_claim.node_id != c.node_id
                    {
                        return Err(Error::Forbidden);
                    }
                    for sha in [&c.context_sha256, &c.response_sha256, &c.prefix_sha256] {
                        digest(sha)?;
                    }
                    let model = self
                        .attempts
                        .get(&c.model_turn_id)
                        .ok_or(Error::Forbidden)?;
                    if model.claim != c.model_claim
                        || !matches!(
                            model.operation,
                            WorkflowOperation::Model { .. } | WorkflowOperation::Preparation { .. }
                        )
                        || model.response_sha256.as_ref() != Some(&c.response_sha256)
                    {
                        return Err(Error::Forbidden);
                    }
                }
                self.settled_cursors.extend(cursors.clone());
                for node in self
                    .nodes
                    .values_mut()
                    .filter(|n| n.state == WorkflowNodeState::Interrupted)
                {
                    node.state = if let Some(output) = completed.get(&node.plan.id) {
                        if output.len() > 65_536 {
                            return Err(Error::ContextLimit);
                        }
                        WorkflowNodeState::Completed {
                            output: output.clone(),
                            sha256: hash(output.as_bytes()),
                        }
                    } else {
                        WorkflowNodeState::Pending
                    };
                }
            }
            Decision::ExternalReconciled {
                id,
                response_sha256,
                succeeded,
                evidence_sha256,
            } => {
                digest(response_sha256)?;
                digest(evidence_sha256)?;
                let attempt = self.attempts.get_mut(id).ok_or(Error::NotFound)?;
                if !matches!(attempt.operation, WorkflowOperation::Tool { .. })
                    || self.inputs.contains_key(id)
                    || self.nodes[&attempt.claim.node_id].state != WorkflowNodeState::Interrupted
                    || attempt.response_sha256.is_some()
                    || attempt
                        .outcome
                        .as_ref()
                        .is_some_and(|o| o != &WorkflowOutcome::Uncertain)
                {
                    return Err(Error::Conflict);
                }
                attempt.outcome = Some(if *succeeded {
                    WorkflowOutcome::Succeeded
                } else {
                    WorkflowOutcome::Failed
                });
                attempt.response_sha256 = Some(response_sha256.clone());
                attempt.finished_at = Some(at);
            }
            Decision::Recover => {
                for node in self.nodes.values_mut() {
                    if matches!(node.state, WorkflowNodeState::Running { .. }) {
                        node.state = WorkflowNodeState::Interrupted;
                    }
                }
            }
        }
        Ok(())
    }
    fn propagate_failure(&mut self, plan: &WorkflowExecutionGraph<'_>) {
        loop {
            let failed: BTreeSet<_> = self
                .nodes
                .iter()
                .filter(|(_, n)| {
                    matches!(
                        n.state,
                        WorkflowNodeState::Failed { .. } | WorkflowNodeState::Blocked
                    )
                })
                .map(|(id, _)| id.clone())
                .collect();
            let blocked: Vec<_> = self
                .nodes
                .iter()
                .filter(|(_, n)| {
                    matches!(
                        n.state,
                        WorkflowNodeState::Pending
                            | WorkflowNodeState::WaitingChildren
                            | WorkflowNodeState::WaitingTimer { .. }
                    ) && (self.composition_stopped(plan, &n.plan.id)
                        || (!failed.is_empty() && plan.definition.failure == WorkflowFailure::Stop)
                        || n.plan
                            .parent
                            .as_ref()
                            .is_some_and(|parent| failed.contains(parent))
                        || !n.plan.dependencies.is_disjoint(&failed)
                        || self.nodes.values().any(|child| {
                            child.plan.parent.as_ref() == Some(&n.plan.id)
                                && failed.contains(&child.plan.id)
                        }))
                })
                .map(|(id, _)| id.clone())
                .collect();
            if blocked.is_empty() {
                break;
            }
            for id in blocked {
                self.nodes.get_mut(&id).expect("checked node").state = WorkflowNodeState::Blocked;
            }
        }
    }
}
fn terminal_node(state: &WorkflowNodeState) -> bool {
    matches!(
        state,
        WorkflowNodeState::Completed { .. }
            | WorkflowNodeState::Skipped { .. }
            | WorkflowNodeState::Failed { .. }
            | WorkflowNodeState::Blocked
    )
}
fn digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return invalid("Invalid workflow request digest");
    }
    Ok(())
}
