//! Native workflow planning primitives. These do not authorize or perform IO.
//! The application service must persist each mutation before using its one-shot
//! admission and continue to enforce the shared policy/tool journals at dispatch.
mod progress;
mod review;
pub use review::*;
mod document_steps;
pub use document_steps::*;
mod resources;
pub use resources::WorkflowWorkspaceBinding;
mod composition;
mod control;
pub mod dataflow;
pub use composition::*;
mod embedded;
pub use embedded::*;
mod iteration;
pub use iteration::*;
mod iteration_graph;
pub use iteration_graph::*;
mod iteration_embedded;
use crate::{Error, Result, canonical_hash, valid_text, validate_budgets};
pub use control::*;
use cowork_contracts::{Budgets, ProviderProfile, StructuredOutput};
pub use dataflow::*;
pub use iteration_embedded::*;
pub use progress::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRights {
    /// Source selection only; compiled execution rights contain concrete IDs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub current_workspace: bool,
    pub tools: BTreeSet<String>,
    pub folders: BTreeSet<String>,
}
impl WorkflowRights {
    pub fn intersect(&self, other: &Self) -> Self {
        Self {
            current_workspace: self.current_workspace && other.current_workspace,
            tools: self.tools.intersection(&other.tools).cloned().collect(),
            folders: self.folders.intersection(&other.folders).cloned().collect(),
        }
    }
    fn validate(&self) -> Result<()> {
        if self.tools.len() > 128 || self.folders.len() > 128 {
            return invalid("Workflow rights exceed supported limits");
        }
        for value in self.tools.iter().chain(&self.folders) {
            valid_text(value, 128)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowMode {
    Sequential,
    Parallel {
        max_parallel: u16,
    },
    Hierarchical {
        max_parallel: u16,
        max_depth: u16,
        max_delegations: u16,
        /// A fixed manager step runs once after all declared worker steps.
        /// Omission preserves the existing bounded dynamic-delegation contract.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        manager_step_id: Option<String>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowFailure {
    Stop,
    ContinueIndependent,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRole {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_config: Option<cowork_contracts::EffectiveAgentConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding: Option<cowork_contracts::ModelRegistryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<StructuredOutput>,
    pub id: String,
    pub instructions: String,
    pub provider_id: String,
    pub provider_revision: i64,
    pub rights: WorkflowRights,
    pub budgets: Budgets,
    pub delegate_to: BTreeSet<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStep {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<WorkflowStepExecution>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub zero_context: bool,
    pub id: String,
    pub role_id: String,
    pub goal: String,
    pub dependencies: Vec<String>,
    pub rights: WorkflowRights,
    pub budgets: Budgets,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowStepExecution {
    WebFetch { url: String },
    WebSearch { query: String },
}
impl WorkflowStepExecution {
    pub fn browser_arguments(&self) -> Result<serde_json::Value> {
        match self {
            Self::WebFetch { url } => {
                valid_text(url, 8192)?;
                let scheme = url
                    .strip_prefix("https://")
                    .or_else(|| url.strip_prefix("http://"))
                    .ok_or_else(|| Error::Validation("Web fetch requires an HTTP(S) URL".into()))?;
                if scheme
                    .split('/')
                    .next()
                    .is_none_or(|host| host.is_empty() || host.contains('@'))
                    || url.chars().any(char::is_control)
                {
                    return invalid("Invalid pipeline URL");
                }
                Ok(serde_json::json!({"operation":"fetch_text","url":url}))
            }
            Self::WebSearch { query } => {
                valid_text(query, 8000)?;
                Ok(
                    serde_json::json!({"operation":"search","url":"https://html.duckduckgo.com/html/","text":query}),
                )
            }
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDefinition {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub documents: BTreeMap<String, WorkflowDocumentStep>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub iterations: BTreeMap<String, WorkflowEmbeddedIteration>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub subflows: BTreeMap<String, WorkflowEmbeddedSubflow>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub controls: BTreeMap<String, WorkflowControlStep>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub gates: BTreeMap<String, WorkflowGate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataflow: Option<Box<WorkflowDataflow>>,
    pub format_version: u32,
    pub name: String,
    pub mode: WorkflowMode,
    pub failure: WorkflowFailure,
    pub rights: WorkflowRights,
    pub budgets: Budgets,
    pub roles: Vec<WorkflowRole>,
    pub steps: Vec<WorkflowStep>,
}
/// Supplied by the trusted application service, after policy intersections.
/// Deserializing this type is not evidence that these grants are authorized.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowScope {
    /// Trusted logical identity of the current order's managed workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_folder: Option<WorkflowWorkspaceBinding>,
    pub rights: WorkflowRights,
    pub budgets: Budgets,
    pub max_parallel: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRolePlan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_config: Option<cowork_contracts::EffectiveAgentConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding: Option<cowork_contracts::ModelRegistryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<StructuredOutput>,
    pub id: String,
    pub instructions: String,
    pub provider: ProviderProfile,
    pub rights: WorkflowRights,
    pub budgets: Budgets,
    pub delegate_to: BTreeSet<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNodePlan {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<String>,
    pub goal: String,
    pub dependencies: BTreeSet<String>,
    pub rights: WorkflowRights,
    pub budgets: Budgets,
    pub depth: u16,
    pub parent: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPlan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_definition: Option<Box<WorkflowDefinition>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub iteration_definition: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration_program: Option<Box<WorkflowIterationProgram>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub iterations: BTreeMap<String, WorkflowCompiledIteration>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub composition_definition: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composition: Option<Box<WorkflowComposition>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub composition_scopes: BTreeMap<String, WorkflowCompositionScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crew_review: Option<String>,
    pub definition: WorkflowDefinition,
    pub scope: WorkflowScope,
    pub profiles: Vec<ProviderProfile>,
    pub source_sha256: String,
    pub sha256: String,
    pub roles: BTreeMap<String, WorkflowRolePlan>,
    pub nodes: BTreeMap<String, WorkflowNodePlan>,
    pub budgets: Budgets,
    pub max_parallel: u16,
    pub max_depth: u16,
    pub max_delegations: u16,
}
impl WorkflowDefinition {
    /// Library validation is independent of current providers/credentials. Start
    /// compilation additionally resolves and freezes every declared provider.
    pub fn validate_definition(&self) -> Result<()> {
        self.validate_definition_limit(64)
    }
    fn validate_definition_limit(&self, max_steps: usize) -> Result<()> {
        if self.format_version == 5 || !self.iterations.is_empty() {
            return self
                .execution_definition()?
                .validate_definition_limit(max_steps);
        }
        if self.format_version == 4 || !self.subflows.is_empty() {
            return self
                .execution_definition()?
                .validate_definition_limit(max_steps);
        }
        if !matches!(self.format_version, 1..=3)
            || (self.roles.is_empty() && self.format_version != 3)
            || self.roles.len() > 32
            || (self.steps.is_empty() && self.controls.is_empty() && self.documents.is_empty())
            || self
                .steps
                .len()
                .saturating_add(self.controls.len())
                .saturating_add(self.documents.len())
                > max_steps
        {
            return invalid("Invalid workflow definition limits");
        }
        valid_text(&self.name, 200)?;
        validate_budgets(&self.budgets)?;
        self.rights.validate()?;
        let hierarchical = matches!(self.mode, WorkflowMode::Hierarchical { .. });
        match self.mode {
            WorkflowMode::Sequential => {}
            WorkflowMode::Parallel { max_parallel } if (1..=32).contains(&max_parallel) => {}
            WorkflowMode::Hierarchical {
                max_parallel,
                max_depth,
                max_delegations,
                ..
            } if (1..=32).contains(&max_parallel)
                && (1..=8).contains(&max_depth)
                && (1..=64).contains(&max_delegations) => {}
            _ => return invalid("Invalid workflow mode limits"),
        }
        let mut roles = BTreeSet::new();
        for role in &self.roles {
            if let Some(output) = &role.output {
                crate::structured_output::validate_schema(output)?;
            }
            identifier(&role.id)?;
            valid_text(&role.instructions, 256_000)?;
            valid_text(&role.provider_id, 128)?;
            role.rights.validate()?;
            if let Some(config) = &role.agent_config {
                crate::validate_agent_values(config)?;
            }
            validate_budgets(&role.budgets)?;
            if role.provider_revision <= 0
                || !roles.insert(role.id.clone())
                || role.delegate_to.len() > 32
                || (!hierarchical && !role.delegate_to.is_empty())
            {
                return invalid("Invalid workflow role");
            }
        }
        if self.roles.iter().any(|r| !r.delegate_to.is_subset(&roles)) {
            return invalid("Missing workflow delegation role");
        }
        let mut nodes = BTreeMap::new();
        let mut previous: Option<String> = None;
        for step in &self.steps {
            if let Some(execution) = &step.execution {
                execution.browser_arguments()?;
                if !step.rights.tools.contains("browser") {
                    return invalid("Pipeline web steps require explicit browser rights");
                }
            }
            identifier(&step.id)?;
            valid_text(&step.goal, 16_000)?;
            step.rights.validate()?;
            validate_budgets(&step.budgets)?;
            let mut dependencies: BTreeSet<_> = step.dependencies.iter().cloned().collect();
            if step.id.starts_with("delegated-")
                || !roles.contains(&step.role_id)
                || dependencies.len() != step.dependencies.len()
            {
                return invalid("Invalid workflow step");
            }
            if matches!(self.mode, WorkflowMode::Sequential)
                && let Some(before) = previous
            {
                dependencies.insert(before);
            }
            let node = WorkflowNodePlan {
                id: step.id.clone(),
                role_id: Some(step.role_id.clone()),
                goal: step.goal.clone(),
                dependencies,
                rights: step.rights.clone(),
                budgets: step.budgets.clone(),
                depth: 0,
                parent: None,
            };
            if nodes.insert(step.id.clone(), node).is_some() {
                return invalid("Duplicate workflow step");
            }
            previous = Some(step.id.clone());
        }
        control::add_nodes(self, None, &mut nodes)?;
        document_steps::add_nodes(self, None, &mut nodes)?;
        static_hierarchy(self, &mut nodes)?;
        check_dag(&nodes)?;
        dataflow::validate_definition(self, &nodes)?;
        control::validate(self, &nodes)
    }
}
impl WorkflowNodePlan {
    pub fn role(&self) -> Result<&str> {
        self.role_id.as_deref().ok_or(Error::Forbidden)
    }
}
impl WorkflowPlan {
    pub fn compile(
        definition: &WorkflowDefinition,
        scope: &WorkflowScope,
        profiles: &[ProviderProfile],
    ) -> Result<Self> {
        Self::compile_internal(definition, scope, profiles, None)
    }
    /// The private Crew adapter retains all 64 original tasks and adds exactly
    /// one dependent manager review. Generic workflow inputs retain their limit.
    pub fn compile_crew_review(
        definition: &WorkflowDefinition,
        scope: &WorkflowScope,
        profiles: &[ProviderProfile],
    ) -> Result<Self> {
        let review = "crew-manager-review";
        let canonical = |id: &str, prefix: &str| {
            id.strip_prefix(prefix).is_some_and(|hash| {
                hash.len() == 40
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
        };
        if definition.steps.len() != 65
            || !matches!(&definition.mode, WorkflowMode::Hierarchical { manager_step_id: Some(id), .. } if id == review)
            || definition.steps.last().is_none_or(|step| step.id != review)
            || definition.steps[..64]
                .iter()
                .any(|step| !canonical(&step.id, "crew-task-"))
            || definition
                .roles
                .iter()
                .any(|role| !canonical(&role.id, "crew-role-"))
            || definition.steps[64]
                .dependencies
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
                != definition.steps[..64]
                    .iter()
                    .map(|step| step.id.clone())
                    .collect()
        {
            return invalid("Invalid private Crew manager review");
        }
        Self::compile_internal(definition, scope, profiles, Some(review.into()))
    }
    fn compile_internal(
        definition: &WorkflowDefinition,
        scope: &WorkflowScope,
        profiles: &[ProviderProfile],
        crew_review: Option<String>,
    ) -> Result<Self> {
        if scope.rights.current_workspace {
            return Err(Error::Forbidden);
        }
        if definition.uses_current_workspace() {
            let resolved = definition.resolve_current_workspace(scope)?;
            let mut plan = Self::compile_internal(&resolved, scope, profiles, crew_review)?;
            plan.resource_definition = Some(Box::new(definition.clone()));
            plan.source_sha256 = canonical_hash(definition);
            plan.sha256 = plan.digest();
            return Ok(plan);
        }
        if definition.format_version == 5 || !definition.iterations.is_empty() {
            if crew_review.is_some() {
                return Err(Error::Forbidden);
            }
            let source = definition.iteration_source()?;
            let mut plan = Self::compile_iterations(&source, scope, profiles)?;
            plan.iteration_definition = true;
            plan.source_sha256 = canonical_hash(definition);
            plan.sha256 = plan.digest();
            return Ok(plan);
        }
        if definition.format_version == 4 || !definition.subflows.is_empty() {
            if crew_review.is_some() {
                return Err(Error::Forbidden);
            }
            let source = definition.composition_source()?;
            let mut plan = Self::compile_composition(&source, scope, profiles)?;
            plan.composition_definition = true;
            plan.source_sha256 = canonical_hash(definition);
            plan.sha256 = plan.digest();
            return Ok(plan);
        }
        let max_steps = if crew_review.is_some() { 65 } else { 64 };
        definition.validate_definition_limit(max_steps)?;
        if !matches!(definition.format_version, 1..=3)
            || (definition.roles.is_empty() && definition.format_version != 3)
            || definition.roles.len() > 32
            || (definition.steps.is_empty()
                && definition.controls.is_empty()
                && definition.documents.is_empty())
            || definition
                .steps
                .len()
                .saturating_add(definition.controls.len())
                .saturating_add(definition.documents.len())
                > max_steps
            || profiles.len() > 128
            || scope.max_parallel == 0
            || scope.max_parallel > 32
        {
            return invalid("Invalid workflow format or supported limits");
        }
        valid_text(&definition.name, 200)?;
        validate_budgets(&definition.budgets)?;
        validate_budgets(&scope.budgets)?;
        definition.rights.validate()?;
        scope.rights.validate()?;
        let (parallel, depth, delegations) = match definition.mode {
            WorkflowMode::Sequential => (1, 0, 0),
            WorkflowMode::Parallel { max_parallel } => (max_parallel, 0, 0),
            WorkflowMode::Hierarchical {
                max_parallel,
                max_depth,
                max_delegations,
                ..
            } => {
                if !(1..=8).contains(&max_depth) || !(1..=64).contains(&max_delegations) {
                    return invalid("Invalid workflow delegation limits");
                }
                (max_parallel, max_depth, max_delegations)
            }
        };
        if !(1..=32).contains(&parallel) {
            return invalid("Invalid workflow parallel limit");
        }
        let budgets = minimum_budgets(&definition.budgets, &scope.budgets);
        let rights = definition.rights.intersect(&scope.rights);
        let mut provider_map = BTreeMap::new();
        for p in profiles {
            if provider_map.insert(&p.id, p).is_some() {
                return invalid("Duplicate workflow provider");
            }
        }
        let mut roles = BTreeMap::new();
        let mut used_profiles = BTreeMap::new();
        // Existing plans keep every original profile byte. New document graphs
        // require only roles reached by an actual role-backed step/delegation.
        let selected_roles = document_steps::used_roles(definition);
        for role in definition
            .roles
            .iter()
            .filter(|role| definition.documents.is_empty() || selected_roles.contains(&role.id))
        {
            identifier(&role.id)?;
            valid_text(&role.instructions, 256_000)?;
            role.rights.validate()?;
            validate_budgets(&role.budgets)?;
            let provider = provider_map
                .get(&role.provider_id)
                .ok_or(Error::Provider("profile_missing".into()))?;
            if provider.revision != role.provider_revision || provider.revision <= 0 {
                return Err(Error::Conflict);
            }
            if !provider.credential_ready {
                return Err(Error::Provider("credential_missing".into()));
            }
            if provider.context_tokens < 1024 || provider.context_tokens > 2_000_000 {
                return invalid("Invalid workflow context limit");
            }
            let limits = minimum_budgets(&budgets, &role.budgets);
            if limits.max_output_tokens >= provider.context_tokens
                || (role.model_binding.is_none()
                    && limits.max_total_tokens < u64::from(provider.context_tokens))
            {
                return Err(Error::BudgetConfiguration);
            }
            if depth == 0 && !role.delegate_to.is_empty() {
                return invalid("Delegation requires hierarchical mode");
            }
            if role.delegate_to.len() > 32 {
                return invalid("Too many delegation roles");
            }
            let compiled = WorkflowRolePlan {
                agent_config: role.agent_config.clone(),
                model_binding: role.model_binding.clone(),
                output: role.output.clone(),
                id: role.id.clone(),
                instructions: role.instructions.clone(),
                provider: (*provider).clone(),
                rights: {
                    let mut effective = rights.intersect(&role.rights);
                    if let Some(filter) = role
                        .agent_config
                        .as_ref()
                        .and_then(|c| c.tool_filter.as_ref())
                    {
                        effective.tools.retain(|tool| filter.contains(tool));
                    }
                    if role
                        .agent_config
                        .as_ref()
                        .and_then(|c| c.access_mode.as_ref())
                        .is_some_and(|mode| matches!(mode, cowork_contracts::AccessMode::ReadOnly))
                    {
                        effective.tools.retain(|tool| crate::read_only_tool(tool));
                    }
                    effective
                },
                budgets: limits,
                delegate_to: role.delegate_to.clone(),
            };
            if roles.insert(role.id.clone(), compiled).is_some() {
                return invalid("Duplicate workflow role");
            }
            used_profiles.insert(provider.id.clone(), (*provider).clone());
        }
        for role in roles.values() {
            if role
                .delegate_to
                .iter()
                .any(|target| !roles.contains_key(target))
            {
                return invalid("Missing workflow delegation role");
            }
        }
        let mut nodes = BTreeMap::new();
        let mut previous: Option<String> = None;
        for step in &definition.steps {
            identifier(&step.id)?;
            if step.id.starts_with("delegated-") {
                return invalid("Reserved workflow step identifier");
            }
            valid_text(&step.goal, 16000)?;
            step.rights.validate()?;
            validate_budgets(&step.budgets)?;
            let role = roles
                .get(&step.role_id)
                .ok_or_else(|| Error::Validation("Missing workflow role".into()))?;
            let mut dependencies: BTreeSet<_> = step.dependencies.iter().cloned().collect();
            if dependencies.len() != step.dependencies.len() {
                return invalid("Duplicate workflow dependency");
            }
            if matches!(definition.mode, WorkflowMode::Sequential)
                && let Some(before) = previous
            {
                dependencies.insert(before);
            }
            let limits = minimum_budgets(&role.budgets, &step.budgets);
            if role.model_binding.is_none()
                && limits.max_total_tokens < u64::from(role.provider.context_tokens)
            {
                return Err(Error::BudgetConfiguration);
            }
            let node = WorkflowNodePlan {
                id: step.id.clone(),
                role_id: Some(role.id.clone()),
                goal: step.goal.clone(),
                dependencies,
                rights: role.rights.intersect(&step.rights),
                budgets: limits,
                depth: 0,
                parent: None,
            };
            if nodes.insert(step.id.clone(), node).is_some() {
                return invalid("Duplicate workflow step");
            }
            previous = Some(step.id.clone());
        }
        control::add_nodes(definition, Some(scope), &mut nodes)?;
        document_steps::add_nodes(definition, Some(scope), &mut nodes)?;
        static_hierarchy(definition, &mut nodes)?;
        check_dag(&nodes)?;
        let mut plan = Self {
            resource_definition: None,
            iteration_definition: false,
            iteration_program: None,
            iterations: BTreeMap::new(),
            composition_definition: false,
            composition: None,
            composition_scopes: BTreeMap::new(),
            crew_review,
            definition: definition.clone(),
            scope: scope.clone(),
            profiles: used_profiles.into_values().collect(),
            source_sha256: canonical_hash(definition),
            sha256: String::new(),
            roles,
            nodes,
            budgets,
            max_parallel: parallel.min(scope.max_parallel),
            max_depth: depth,
            max_delegations: delegations,
        };
        plan.sha256 = plan.digest();
        Ok(plan)
    }
    fn digest(&self) -> String {
        let mut copy = self.clone();
        copy.sha256.clear();
        canonical_hash(&copy)
    }
    pub fn validate(&self) -> Result<()> {
        if let Some(source) = &self.resource_definition {
            if !source.uses_current_workspace() {
                return Err(Error::Forbidden);
            }
            let rebuilt = Self::compile_internal(
                source,
                &self.scope,
                &self.profiles,
                self.crew_review.clone(),
            )?;
            return if self.digest() == self.sha256 && rebuilt.sha256 == self.sha256 {
                Ok(())
            } else {
                Err(Error::Forbidden)
            };
        }
        if let Some(source) = &self.iteration_program {
            let rebuilt = if self.iteration_definition {
                Self::compile(&source.source_definition()?, &self.scope, &self.profiles)?
            } else {
                Self::compile_iterations(source, &self.scope, &self.profiles)?
            };
            return if self.digest() == self.sha256 && rebuilt.sha256 == self.sha256 {
                Ok(())
            } else {
                Err(Error::Forbidden)
            };
        }
        if let Some(composition) = &self.composition {
            let compiled = if self.composition_definition {
                Self::compile(
                    &composition.source_definition()?,
                    &self.scope,
                    &self.profiles,
                )?
            } else {
                Self::compile_composition(composition, &self.scope, &self.profiles)?
            };
            return if self.digest() == self.sha256 && compiled.sha256 == self.sha256 {
                Ok(())
            } else {
                Err(Error::Forbidden)
            };
        }
        let compiled = match self.crew_review.as_deref() {
            None => Self::compile(&self.definition, &self.scope, &self.profiles)?,
            Some("crew-manager-review") => {
                Self::compile_crew_review(&self.definition, &self.scope, &self.profiles)?
            }
            Some(_) => return Err(Error::Forbidden),
        };
        if self.digest() != self.sha256 || compiled.sha256 != self.sha256 {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
pub(super) fn minimum_budgets(a: &Budgets, b: &Budgets) -> Budgets {
    Budgets {
        max_steps: a.max_steps.min(b.max_steps),
        max_seconds: a.max_seconds.min(b.max_seconds),
        max_output_tokens: a.max_output_tokens.min(b.max_output_tokens),
        max_total_tokens: a.max_total_tokens.min(b.max_total_tokens),
    }
}
fn check_dag(nodes: &BTreeMap<String, WorkflowNodePlan>) -> Result<()> {
    for node in nodes.values() {
        if node.dependencies.contains(&node.id)
            || node.dependencies.iter().any(|d| !nodes.contains_key(d))
        {
            return invalid("Missing or self-referencing workflow dependency");
        }
    }
    let mut completed = BTreeSet::new();
    while completed.len() < nodes.len() {
        let next: Vec<_> = nodes
            .values()
            .filter(|n| !completed.contains(&n.id) && n.dependencies.is_subset(&completed))
            .map(|n| n.id.clone())
            .collect();
        if next.is_empty() {
            return invalid("Workflow contains a cycle");
        }
        completed.extend(next);
    }
    Ok(())
}
fn static_hierarchy(
    definition: &WorkflowDefinition,
    nodes: &mut BTreeMap<String, WorkflowNodePlan>,
) -> Result<()> {
    let WorkflowMode::Hierarchical {
        manager_step_id: Some(manager),
        ..
    } = &definition.mode
    else {
        return Ok(());
    };
    identifier(manager)?;
    if nodes.len() < 2 {
        return invalid("Static hierarchy requires workers");
    }
    let workers: BTreeSet<_> = nodes.keys().filter(|id| *id != manager).cloned().collect();
    let rollup = nodes
        .get_mut(manager)
        .ok_or_else(|| Error::Validation("Missing hierarchy manager step".into()))?;
    rollup.dependencies.extend(workers);
    Ok(())
}
pub(super) fn identifier(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return invalid("Invalid workflow identifier");
    }
    Ok(())
}
pub(super) fn invalid<T>(message: &str) -> Result<T> {
    Err(Error::Validation(message.into()))
}
