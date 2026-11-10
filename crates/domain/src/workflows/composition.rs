//! Frozen typed subflows lowered into one parent's journal and authority scope.
//! This private compiler foundation is not yet a public start or editor format.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowComposition {
    pub format_version: u32,
    pub workflow: WorkflowDefinition,
    pub calls: BTreeMap<String, WorkflowSubflowCall>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSubflowCall {
    pub name: String,
    pub dependencies: Vec<String>,
    pub body: Box<WorkflowComposition>,
    pub result: WorkflowOutputRef,
    pub rights: WorkflowRights,
    pub budgets: Budgets,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCompositionScope {
    pub entry_node: String,
    pub max_parallel: u16,
    pub failure: WorkflowFailure,
}

struct Placement {
    parent: String,
    rights: WorkflowRights,
    budgets: Budgets,
}
struct Lowered {
    definition: WorkflowDefinition,
    scopes: BTreeMap<String, WorkflowCompositionScope>,
    placements: BTreeMap<String, Placement>,
}

fn invalid_composition<T>() -> Result<T> {
    Err(Error::Validation("workflow_composition_invalid".into()))
}
fn source_id(id: &str) -> Result<()> {
    identifier(id)?;
    if id.starts_with("composed-") || id.starts_with("delegated-") {
        return invalid_composition();
    }
    Ok(())
}
fn scoped(path: &[String], kind: &str, id: &str) -> String {
    if path.is_empty() {
        return id.into();
    }
    format!(
        "composed-{}",
        &canonical_hash(&serde_json::json!([path, kind, id]))[..40]
    )
}
fn mapped(ids: &BTreeMap<String, String>, id: &str) -> Result<String> {
    ids.get(id)
        .cloned()
        .ok_or_else(|| Error::Validation("workflow_composition_reference_missing".into()))
}
fn binding(
    source: &WorkflowValueBinding,
    ids: &BTreeMap<String, String>,
    entry: Option<&str>,
) -> Result<WorkflowValueBinding> {
    let mut result = source.clone();
    result.value = match &source.value {
        WorkflowDataSource::Literal { .. } => source.value.clone(),
        WorkflowDataSource::Input { pointer } => match entry {
            Some(entry) => WorkflowDataSource::Step {
                step_id: entry.into(),
                pointer: pointer.clone(),
            },
            None => source.value.clone(),
        },
        WorkflowDataSource::Step { step_id, pointer } => WorkflowDataSource::Step {
            step_id: mapped(ids, step_id)?,
            pointer: pointer.clone(),
        },
    };
    Ok(result)
}

impl Lowered {
    fn append(
        &mut self,
        program: &WorkflowComposition,
        path: &[String],
        entry: Option<&str>,
        parent: Option<&str>,
        rights: &WorkflowRights,
        budgets: &Budgets,
    ) -> Result<BTreeMap<String, String>> {
        if program.format_version != 1
            || path.len() > 4
            || self.scopes.len() > 16
            || program.calls.len() > 16
            || program.workflow.roles.len() > 32
            || program.workflow.steps.len()
                + program.workflow.controls.len()
                + program.workflow.documents.len()
                > 64
        {
            return invalid_composition();
        }
        let source = &program.workflow;
        if program.calls.is_empty() {
            source.validate_definition()?;
        } else if source.format_version != 3 {
            return invalid_composition();
        }
        if !source.subflows.is_empty()
            || !source.iterations.is_empty()
            || !matches!(source.format_version, 2 | 3)
            || matches!(source.mode, WorkflowMode::Hierarchical { .. })
            || !program.calls.is_empty() && !matches!(source.mode, WorkflowMode::Parallel { .. })
        {
            return invalid_composition();
        }
        valid_text(&source.name, 200)?;
        validate_budgets(&source.budgets)?;
        source.rights.validate()?;
        let data = source
            .dataflow
            .as_ref()
            .ok_or_else(|| Error::Validation("workflow_data_input_required".into()))?;
        data.input.validate_type()?;
        if !matches!(data.input, WorkflowDataType::Object { .. }) {
            return invalid_composition();
        }
        let effective_rights = rights.intersect(&source.rights);
        let effective_budgets = minimum_budgets(budgets, &source.budgets);
        let mut ids = BTreeMap::new();
        for id in source
            .steps
            .iter()
            .map(|s| &s.id)
            .chain(source.controls.keys())
            .chain(source.documents.keys())
            .chain(program.calls.keys())
        {
            source_id(id)?;
            if ids.insert(id.clone(), scoped(path, "node", id)).is_some() {
                return invalid_composition();
            }
        }
        if ids.is_empty() || ids.len() > 64 || data.steps.keys().ne(ids.keys()) {
            return invalid_composition();
        }
        let mut roles = BTreeMap::new();
        for role in &source.roles {
            source_id(&role.id)?;
            if !role.delegate_to.is_empty()
                || roles
                    .insert(role.id.clone(), scoped(path, "role", &role.id))
                    .is_some()
            {
                return invalid_composition();
            }
            let mut role = role.clone();
            role.id = mapped(&roles, &role.id)?;
            role.rights = role.rights.intersect(&effective_rights);
            role.budgets = minimum_budgets(&role.budgets, &effective_budgets);
            self.definition.roles.push(role);
        }
        let dependencies = |values: &[String]| -> Result<Vec<String>> {
            let mut result = values
                .iter()
                .map(|id| mapped(&ids, id))
                .collect::<Result<Vec<_>>>()?;
            if let Some(entry) = entry {
                result.push(entry.into());
            }
            Ok(result)
        };
        let mut previous = None;
        for original in &source.steps {
            let mut step = original.clone();
            step.id = mapped(&ids, &step.id)?;
            step.role_id = mapped(&roles, &step.role_id)?;
            step.dependencies = dependencies(&step.dependencies)?;
            if matches!(source.mode, WorkflowMode::Sequential) {
                if let Some(before) = previous
                    && !step.dependencies.contains(&before)
                {
                    step.dependencies.push(before);
                }
                previous = Some(step.id.clone());
            }
            step.rights = step.rights.intersect(&effective_rights);
            step.budgets = minimum_budgets(&step.budgets, &effective_budgets);
            self.definition.steps.push(step);
        }
        for (id, original) in &source.documents {
            let mut step = original.clone();
            step.dependencies = dependencies(&step.dependencies)?;
            step.rights = step.rights.intersect(&effective_rights);
            step.budgets = minimum_budgets(&step.budgets, &effective_budgets);
            if self
                .definition
                .documents
                .insert(mapped(&ids, id)?, step)
                .is_some()
            {
                return invalid_composition();
            }
        }
        for (id, original) in &source.controls {
            let mut step = original.clone();
            step.dependencies = dependencies(&step.dependencies)?;
            match &mut step.operation {
                WorkflowControlOperation::Review { form, rights } => {
                    *rights = rights.intersect(&effective_rights);
                    for source in &mut form.source_steps {
                        *source = mapped(&ids, source)?;
                    }
                }
                WorkflowControlOperation::Select { candidates }
                | WorkflowControlOperation::Collect { candidates } => {
                    for candidate in candidates {
                        candidate.step_id = mapped(&ids, &candidate.step_id)?;
                    }
                }
                _ => {}
            }
            if self
                .definition
                .controls
                .insert(mapped(&ids, id)?, step)
                .is_some()
            {
                return invalid_composition();
            }
        }
        for (id, config) in &data.steps {
            if program.calls.contains_key(id) {
                continue;
            }
            let mut config = config.clone();
            for input in config.inputs.values_mut() {
                *input = binding(input, &ids, entry)?;
            }
            self.definition
                .dataflow
                .as_mut()
                .expect("initialized data")
                .steps
                .insert(mapped(&ids, id)?, config);
        }
        for (id, gate) in &source.gates {
            let target = mapped(&ids, id)?;
            let target = if program.calls.contains_key(id) {
                scoped(
                    &[path, std::slice::from_ref(id)].concat(),
                    "entry",
                    "arguments",
                )
            } else {
                target
            };
            self.definition.gates.insert(
                target,
                WorkflowGate {
                    step_id: mapped(&ids, &gate.step_id)?,
                    when: gate.when,
                },
            );
        }
        if let Some(parent) = parent {
            for id in ids.values() {
                self.placements.insert(
                    id.clone(),
                    Placement {
                        parent: parent.into(),
                        rights: effective_rights.clone(),
                        budgets: effective_budgets.clone(),
                    },
                );
            }
        }
        for (id, call) in &program.calls {
            if self.scopes.len() >= 16 {
                return invalid_composition();
            }
            valid_text(&call.name, 200)?;
            call.rights.validate()?;
            validate_budgets(&call.budgets)?;
            let limits = minimum_budgets(
                &effective_budgets,
                &minimum_budgets(&call.budgets, &call.body.workflow.budgets),
            );
            let grants = effective_rights.intersect(&call.rights);
            let exit = mapped(&ids, id)?;
            let next_path = [path, std::slice::from_ref(id)].concat();
            let frame = scoped(&next_path, "entry", "arguments");
            let args = &data.steps[id];
            let args_type = WorkflowDataType::Object {
                fields: args
                    .inputs
                    .iter()
                    .map(|(key, b)| (key.clone(), b.value_type.clone()))
                    .collect(),
            };
            if call
                .body
                .workflow
                .dataflow
                .as_ref()
                .is_none_or(|d| d.input != args_type)
            {
                return invalid_composition();
            }
            let parallel = match call.body.workflow.mode {
                WorkflowMode::Sequential => 1,
                WorkflowMode::Parallel { max_parallel } if (1..=32).contains(&max_parallel) => {
                    max_parallel
                }
                _ => return invalid_composition(),
            };
            self.scopes.insert(
                exit.clone(),
                WorkflowCompositionScope {
                    entry_node: frame.clone(),
                    max_parallel: parallel,
                    failure: call.body.workflow.failure.clone(),
                },
            );
            let config = WorkflowStepData {
                context_policy: WorkflowDataContext::Inherit,
                inputs: args
                    .inputs
                    .iter()
                    .map(|(key, b)| Ok((key.clone(), binding(b, &ids, entry)?)))
                    .collect::<Result<_>>()?,
                output: args_type,
            };
            self.definition.controls.insert(
                frame.clone(),
                WorkflowControlStep {
                    name: call.name.clone(),
                    dependencies: dependencies(&call.dependencies)?,
                    operation: WorkflowControlOperation::Compose,
                },
            );
            self.definition
                .dataflow
                .as_mut()
                .expect("initialized data")
                .steps
                .insert(frame.clone(), config);
            self.placements.insert(
                frame.clone(),
                Placement {
                    parent: exit.clone(),
                    rights: grants.clone(),
                    budgets: limits.clone(),
                },
            );
            let body_ids = self.append(
                &call.body,
                &next_path,
                Some(&frame),
                Some(&exit),
                &grants,
                &limits,
            )?;
            let result = mapped(&body_ids, &call.result.step_id)?;
            let result_type = self
                .definition
                .dataflow
                .as_ref()
                .expect("initialized data")
                .steps[&result]
                .output
                .at_pointer(&call.result.pointer)?
                .clone();
            if args.output != result_type {
                return invalid_composition();
            }
            let dependencies = body_ids
                .values()
                .cloned()
                .chain(std::iter::once(frame))
                .collect();
            self.definition.controls.insert(
                exit.clone(),
                WorkflowControlStep {
                    name: call.name.clone(),
                    dependencies,
                    operation: WorkflowControlOperation::Select {
                        candidates: vec![WorkflowOutputRef {
                            step_id: result,
                            pointer: call.result.pointer.clone(),
                        }],
                    },
                },
            );
            self.definition
                .dataflow
                .as_mut()
                .expect("initialized data")
                .steps
                .insert(
                    exit.clone(),
                    WorkflowStepData {
                        context_policy: WorkflowDataContext::Inherit,
                        inputs: BTreeMap::new(),
                        output: result_type,
                    },
                );
            // The call's own local limit also constrains its entry and every
            // descendant through their parent link, including token holds.
            self.placements.entry(exit).and_modify(|p| {
                p.rights = p.rights.intersect(&grants);
                p.budgets = minimum_budgets(&p.budgets, &limits);
            });
        }
        if self.definition.steps.len()
            + self.definition.controls.len()
            + self.definition.documents.len()
            > 64
            || self.definition.roles.len() > 32
        {
            return invalid_composition();
        }
        Ok(ids)
    }
}

impl WorkflowPlan {
    pub fn compile_composition(
        program: &WorkflowComposition,
        scope: &WorkflowScope,
        profiles: &[ProviderProfile],
    ) -> Result<Self> {
        if program.calls.is_empty() {
            return invalid_composition();
        }
        let mut definition = program.workflow.clone();
        definition.format_version = 3;
        definition.mode = WorkflowMode::Parallel {
            max_parallel: match program.workflow.mode {
                WorkflowMode::Parallel { max_parallel } => max_parallel,
                _ => return invalid_composition(),
            },
        };
        definition.steps.clear();
        definition.roles.clear();
        definition.controls.clear();
        definition.documents.clear();
        definition.gates.clear();
        let data = definition
            .dataflow
            .as_mut()
            .ok_or_else(|| Error::Validation("workflow_data_input_required".into()))?;
        data.steps.clear();
        let mut lowered = Lowered {
            definition,
            scopes: BTreeMap::new(),
            placements: BTreeMap::new(),
        };
        lowered.append(
            program,
            &[],
            None,
            None,
            &program.workflow.rights,
            &program.workflow.budgets,
        )?;
        if serde_json::to_vec(program)
            .map_err(|_| Error::Forbidden)?
            .len()
            > 2 * 1024 * 1024
        {
            return invalid_composition();
        }
        let mut plan = Self::compile(&lowered.definition, scope, profiles)?;
        for (id, placement) in lowered.placements {
            let node = plan.nodes.get_mut(&id).ok_or(Error::Forbidden)?;
            node.parent = Some(placement.parent);
            node.rights = node.rights.intersect(&placement.rights);
            node.budgets = minimum_budgets(&node.budgets, &placement.budgets);
        }
        // Top-level call exits do not themselves have a parent placement.
        for (id, call) in &program.calls {
            let node = plan.nodes.get_mut(id).ok_or(Error::Forbidden)?;
            node.budgets = minimum_budgets(
                &node.budgets,
                &minimum_budgets(&call.budgets, &call.body.workflow.budgets),
            );
        }
        plan.composition_scopes = lowered.scopes;
        plan.composition = Some(Box::new(program.clone()));
        plan.source_sha256 = canonical_hash(program);
        plan.sha256 = plan.digest();
        Ok(plan)
    }
}

impl WorkflowComposition {
    pub fn execution_definition(&self) -> Result<WorkflowDefinition> {
        if self.calls.is_empty() {
            return invalid_composition();
        }
        let mut definition = self.workflow.clone();
        definition.format_version = 3;
        definition.steps.clear();
        definition.roles.clear();
        definition.controls.clear();
        definition.documents.clear();
        definition.gates.clear();
        definition.subflows.clear();
        definition.mode = WorkflowMode::Parallel {
            max_parallel: match self.workflow.mode {
                WorkflowMode::Parallel { max_parallel } => max_parallel,
                _ => return invalid_composition(),
            },
        };
        definition
            .dataflow
            .as_mut()
            .ok_or(Error::Forbidden)?
            .steps
            .clear();
        let mut lowered = Lowered {
            definition,
            scopes: BTreeMap::new(),
            placements: BTreeMap::new(),
        };
        lowered.append(
            self,
            &[],
            None,
            None,
            &self.workflow.rights,
            &self.workflow.budgets,
        )?;
        lowered.definition.validate_definition()?;
        Ok(lowered.definition)
    }
}
