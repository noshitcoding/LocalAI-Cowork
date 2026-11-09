//! One immutable source plan, with a replay-derived execution graph. This view
//! has no independent hash, serialization or IO authority and never replaces
//! the original plan binding as new list values become known.
use super::*;
use std::{borrow::Cow, ops::Deref};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowIterationNode {
    pub source: WorkflowIterationSource,
    pub dependencies: Vec<String>,
    pub inputs: BTreeMap<String, WorkflowValueBinding>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowIterationProgram {
    pub format_version: u32,
    pub workflow: WorkflowDefinition,
    pub iterations: BTreeMap<String, WorkflowIterationNode>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCompiledIteration {
    pub capture_node: String,
    pub source: WorkflowIterationSource,
    pub body: Box<WorkflowPlan>,
    pub role_ids: BTreeMap<String, String>,
}
#[derive(Clone, Debug)]
pub struct WorkflowIterationInstance {
    pub index: u16,
    pub entry: String,
    pub exit: String,
}
pub struct WorkflowExecutionGraph<'a> {
    base: &'a WorkflowPlan,
    pub definition: Cow<'a, WorkflowDefinition>,
    pub nodes: Cow<'a, BTreeMap<String, WorkflowNodePlan>>,
    pub composition_scopes: Cow<'a, BTreeMap<String, WorkflowCompositionScope>>,
    pub iteration_instances: BTreeMap<String, Vec<WorkflowIterationInstance>>,
}
impl Deref for WorkflowExecutionGraph<'_> {
    type Target = WorkflowPlan;
    fn deref(&self) -> &Self::Target {
        self.base
    }
}

pub(super) fn generated(kind: &str, scope: &str, id: &str) -> String {
    format!(
        "iteration-{}",
        &canonical_hash(&serde_json::json!([kind, scope, id]))[..40]
    )
}
pub(super) fn original_id(id: &str) -> Result<()> {
    identifier(id)?;
    if ["iteration-", "composed-", "delegated-"]
        .iter()
        .any(|prefix| id.starts_with(prefix))
    {
        return invalid("workflow_iteration_reserved_id");
    }
    Ok(())
}
fn remap(ids: &BTreeMap<String, String>, id: &str) -> Result<String> {
    ids.get(id).cloned().ok_or(Error::Forbidden)
}

impl WorkflowPlan {
    pub fn compile_iterations(
        source: &WorkflowIterationProgram,
        scope: &WorkflowScope,
        profiles: &[ProviderProfile],
    ) -> Result<Self> {
        let mut definition = source.scaffold()?;
        let rights = definition.rights.intersect(&scope.rights);
        let budgets = minimum_budgets(&definition.budgets, &scope.budgets);
        let mut compiled = BTreeMap::new();
        for (id, iteration) in &source.iterations {
            let capture = generated("capture", id, "");
            let local = WorkflowScope {
                workspace_folder: None,
                rights: rights.intersect(&iteration.source.rights),
                budgets: minimum_budgets(&budgets, &iteration.source.budgets),
                max_parallel: scope.max_parallel,
            };
            let body = Self::compile(&iteration.source.body, &local, profiles)?;
            let mut role_ids = BTreeMap::new();
            for role in body
                .definition
                .roles
                .iter()
                .filter(|role| body.roles.contains_key(&role.id))
            {
                let mapped = generated("role", id, &role.id);
                role_ids.insert(role.id.clone(), mapped.clone());
                let actual = &body.roles[&role.id];
                let mut role = role.clone();
                role.id = mapped;
                role.rights = actual.rights.clone();
                role.budgets = actual.budgets.clone();
                definition.roles.push(role);
            }
            compiled.insert(
                id.clone(),
                WorkflowCompiledIteration {
                    capture_node: capture,
                    source: iteration.source.clone(),
                    body: Box::new(body),
                    role_ids,
                },
            );
        }
        if source.workflow.uses_document_steps()
            || source
                .iterations
                .values()
                .any(|iteration| iteration.source.body.uses_document_steps())
        {
            let selected = document_steps::used_roles(&definition);
            let body_roles: BTreeSet<_> = compiled
                .values()
                .flat_map(|iteration| iteration.role_ids.values())
                .cloned()
                .collect();
            definition
                .roles
                .retain(|role| selected.contains(&role.id) || body_roles.contains(&role.id));
        }
        let mut plan = Self::compile(&definition, scope, profiles)?;
        for (id, iteration) in &compiled {
            let exit = plan.nodes.get_mut(id).ok_or(Error::Forbidden)?;
            exit.rights = exit.rights.intersect(&iteration.source.rights);
            exit.budgets = minimum_budgets(&exit.budgets, &iteration.source.budgets);
            let frame = plan
                .nodes
                .get_mut(&iteration.capture_node)
                .ok_or(Error::Forbidden)?;
            frame.parent = Some(id.clone());
            frame.rights = frame.rights.intersect(&iteration.source.rights);
            frame.budgets = minimum_budgets(&frame.budgets, &iteration.source.budgets);
            plan.composition_scopes.insert(
                id.clone(),
                WorkflowCompositionScope {
                    entry_node: iteration.capture_node.clone(),
                    max_parallel: plan.max_parallel,
                    failure: iteration.source.body.failure.clone(),
                },
            );
        }
        plan.iteration_program = Some(Box::new(source.clone()));
        plan.iterations = compiled;
        plan.source_sha256 = canonical_hash(source);
        plan.sha256 = plan.digest();
        Ok(plan)
    }
}

impl<'a> WorkflowExecutionGraph<'a> {
    pub(super) fn derive(
        base: &'a WorkflowPlan,
        manifests: &BTreeMap<String, WorkflowIterationManifest>,
    ) -> Result<Self> {
        let mut graph = Self {
            base,
            definition: Cow::Borrowed(&base.definition),
            nodes: Cow::Borrowed(&base.nodes),
            composition_scopes: Cow::Borrowed(&base.composition_scopes),
            iteration_instances: BTreeMap::new(),
        };
        // Check a conservative serialized-size estimate before multiplying
        // templates. The derived view is ephemeral, but still needs a bound.
        let mut graph_bytes = if manifests.is_empty() {
            0
        } else {
            serde_json::to_vec(&(&base.definition, &base.nodes))
                .map_err(|_| Error::Forbidden)?
                .len()
        };
        for (id, manifest) in manifests {
            let iteration = base.iterations.get(id).ok_or(Error::Forbidden)?;
            let frames = manifest.verify(&iteration.source, id, &manifest.input_sha256)?;
            let expected_nodes = frames
                .len()
                .checked_mul(iteration.body.nodes.len() + 2)
                .and_then(|n| n.checked_add(graph.nodes.len()))
                .ok_or(Error::BudgetExceeded)?;
            if expected_nodes > 4096 {
                return Err(Error::BudgetExceeded);
            }
            let body = &iteration.body;
            let template_bytes = serde_json::to_vec(&(
                &body.definition.steps,
                &body.definition.controls,
                &body.definition.documents,
                &body.definition.gates,
                &body.definition.dataflow,
                &body.nodes,
                &body.composition_scopes,
                &iteration.source.arguments,
            ))
            .map_err(|_| Error::Forbidden)?
            .len();
            let per_item_bytes = template_bytes
                .checked_mul(2)
                .and_then(|bytes| bytes.checked_add(1024 * body.nodes.len() + 2048))
                .ok_or(Error::BudgetExceeded)?;
            graph_bytes = per_item_bytes
                .checked_mul(frames.len())
                .and_then(|bytes| bytes.checked_add(graph_bytes))
                .ok_or(Error::BudgetExceeded)?;
            if graph_bytes > 32 * 1024 * 1024 {
                return Err(Error::BudgetExceeded);
            }
            let mut instances = Vec::with_capacity(frames.len());
            let mut candidates = vec![];
            for (index, item) in manifest.items.iter().enumerate() {
                let frame = frames.item(index)?;
                if serde_json::to_vec(&frame.input)
                    .map_err(|_| Error::Forbidden)?
                    .len()
                    > 65_536
                {
                    return Err(Error::ContextLimit);
                }
                // Pure preflight only. Unknown model results/timer epochs stay unknown.
                iteration.body.validate_wait_inputs(Some(&frame.input))?;
                let entry = generated("arguments", &item.id, "");
                let exit = item.id.clone();
                let ids: BTreeMap<_, _> = iteration
                    .body
                    .nodes
                    .keys()
                    .map(|id| (id.clone(), generated("node", &item.id, id)))
                    .collect();
                let mut inputs = BTreeMap::new();
                let WorkflowDataType::Object { fields } = &iteration
                    .source
                    .body
                    .dataflow
                    .as_ref()
                    .ok_or(Error::Forbidden)?
                    .input
                else {
                    return Err(Error::Forbidden);
                };
                for (name, kind) in fields {
                    // Reuse the one captured input. Copying shared text into every
                    // derived literal would multiply memory before any item runs.
                    let value = match &iteration.source.arguments[name] {
                        WorkflowIterationArgument::Item { pointer } => WorkflowDataSource::Step {
                            step_id: iteration.capture_node.clone(),
                            pointer: format!(
                                "/{}/{}{}",
                                iteration
                                    .source
                                    .collection_input
                                    .replace('~', "~0")
                                    .replace('/', "~1"),
                                index,
                                pointer
                            ),
                        },
                        WorkflowIterationArgument::Input { pointer } => WorkflowDataSource::Step {
                            step_id: iteration.capture_node.clone(),
                            pointer: pointer.clone(),
                        },
                        WorkflowIterationArgument::Index => WorkflowDataSource::Literal {
                            value: serde_json::json!(index),
                        },
                        WorkflowIterationArgument::Literal { value } => {
                            WorkflowDataSource::Literal {
                                value: value.clone(),
                            }
                        }
                    };
                    inputs.insert(
                        name.clone(),
                        WorkflowValueBinding {
                            value_type: kind.clone(),
                            value,
                        },
                    );
                }
                let rights = base.nodes[id]
                    .rights
                    .intersect(&iteration.source.body.rights);
                let budgets =
                    minimum_budgets(&base.nodes[id].budgets, &iteration.source.body.budgets);
                graph.nodes.to_mut().insert(
                    entry.clone(),
                    WorkflowNodePlan {
                        id: entry.clone(),
                        role_id: None,
                        goal: format!("{} · {}", iteration.source.name, index + 1),
                        dependencies: BTreeSet::from([iteration.capture_node.clone()]),
                        rights: rights.clone(),
                        budgets: budgets.clone(),
                        depth: 0,
                        parent: Some(exit.clone()),
                    },
                );
                graph.definition.to_mut().controls.insert(
                    entry.clone(),
                    WorkflowControlStep {
                        name: format!("{} · {}", iteration.source.name, index + 1),
                        dependencies: vec![iteration.capture_node.clone()],
                        operation: WorkflowControlOperation::Compose,
                    },
                );
                graph
                    .definition
                    .to_mut()
                    .dataflow
                    .as_mut()
                    .ok_or(Error::Forbidden)?
                    .steps
                    .insert(
                        entry.clone(),
                        WorkflowStepData {
                            context_policy: WorkflowDataContext::Inherit,
                            inputs,
                            output: iteration
                                .source
                                .body
                                .dataflow
                                .as_ref()
                                .ok_or(Error::Forbidden)?
                                .input
                                .clone(),
                        },
                    );
                for (old, node) in &iteration.body.nodes {
                    let mut node = node.clone();
                    node.id = remap(&ids, old)?;
                    node.role_id = node
                        .role_id
                        .as_ref()
                        .map(|id| remap(&iteration.role_ids, id))
                        .transpose()?;
                    node.dependencies = node
                        .dependencies
                        .iter()
                        .map(|id| remap(&ids, id))
                        .collect::<Result<_>>()?;
                    node.dependencies.insert(entry.clone());
                    node.parent = Some(match &node.parent {
                        Some(parent) => remap(&ids, parent)?,
                        None => exit.clone(),
                    });
                    if graph.nodes.to_mut().insert(node.id.clone(), node).is_some() {
                        return Err(Error::Conflict);
                    }
                }
                for step in &iteration.body.definition.steps {
                    let mut step = step.clone();
                    step.id = remap(&ids, &step.id)?;
                    step.role_id = remap(&iteration.role_ids, &step.role_id)?;
                    step.dependencies =
                        graph.nodes[&step.id].dependencies.iter().cloned().collect();
                    graph.definition.to_mut().steps.push(step);
                }
                for (old, document) in &iteration.body.definition.documents {
                    let mut document = document.clone();
                    let node = remap(&ids, old)?;
                    document.dependencies =
                        graph.nodes[&node].dependencies.iter().cloned().collect();
                    graph.definition.to_mut().documents.insert(node, document);
                }
                for (old, control) in &iteration.body.definition.controls {
                    let mut control = control.clone();
                    let node = remap(&ids, old)?;
                    control.dependencies =
                        graph.nodes[&node].dependencies.iter().cloned().collect();
                    if let WorkflowControlOperation::Select { candidates }
                    | WorkflowControlOperation::Collect { candidates } = &mut control.operation
                    {
                        for candidate in candidates {
                            candidate.step_id = remap(&ids, &candidate.step_id)?;
                        }
                    }
                    graph.definition.to_mut().controls.insert(node, control);
                }
                for (old, gate) in &iteration.body.definition.gates {
                    graph.definition.to_mut().gates.insert(
                        remap(&ids, old)?,
                        WorkflowGate {
                            step_id: remap(&ids, &gate.step_id)?,
                            when: gate.when,
                        },
                    );
                }
                for (old, data) in &iteration
                    .body
                    .definition
                    .dataflow
                    .as_ref()
                    .ok_or(Error::Forbidden)?
                    .steps
                {
                    let mut data = data.clone();
                    for binding in data.inputs.values_mut() {
                        binding.value = match &binding.value {
                            WorkflowDataSource::Input { pointer } => WorkflowDataSource::Step {
                                step_id: entry.clone(),
                                pointer: pointer.clone(),
                            },
                            WorkflowDataSource::Step { step_id, pointer } => {
                                WorkflowDataSource::Step {
                                    step_id: remap(&ids, step_id)?,
                                    pointer: pointer.clone(),
                                }
                            }
                            literal => literal.clone(),
                        };
                    }
                    graph
                        .definition
                        .to_mut()
                        .dataflow
                        .as_mut()
                        .ok_or(Error::Forbidden)?
                        .steps
                        .insert(remap(&ids, old)?, data);
                }
                for (old, scope) in &iteration.body.composition_scopes {
                    graph.composition_scopes.to_mut().insert(
                        remap(&ids, old)?,
                        WorkflowCompositionScope {
                            entry_node: remap(&ids, &scope.entry_node)?,
                            max_parallel: scope.max_parallel,
                            failure: scope.failure.clone(),
                        },
                    );
                }
                let result = remap(&ids, &iteration.source.result.step_id)?;
                let mut dependencies: BTreeSet<_> = ids.values().cloned().collect();
                dependencies.insert(entry.clone());
                graph.nodes.to_mut().insert(
                    exit.clone(),
                    WorkflowNodePlan {
                        id: exit.clone(),
                        role_id: None,
                        goal: format!("{} · {}", iteration.source.name, index + 1),
                        dependencies: dependencies.clone(),
                        rights,
                        budgets,
                        depth: 0,
                        parent: Some(id.clone()),
                    },
                );
                graph.definition.to_mut().controls.insert(
                    exit.clone(),
                    WorkflowControlStep {
                        name: format!("{} · {}", iteration.source.name, index + 1),
                        dependencies: dependencies.into_iter().collect(),
                        operation: WorkflowControlOperation::Select {
                            candidates: vec![WorkflowOutputRef {
                                step_id: result,
                                pointer: iteration.source.result.pointer.clone(),
                            }],
                        },
                    },
                );
                let WorkflowDataType::List { item: output, .. } = frames.output_type() else {
                    return Err(Error::Forbidden);
                };
                graph
                    .definition
                    .to_mut()
                    .dataflow
                    .as_mut()
                    .ok_or(Error::Forbidden)?
                    .steps
                    .insert(
                        exit.clone(),
                        WorkflowStepData {
                            context_policy: WorkflowDataContext::Inherit,
                            inputs: BTreeMap::new(),
                            output: output.as_ref().clone(),
                        },
                    );
                graph.composition_scopes.to_mut().insert(
                    exit.clone(),
                    WorkflowCompositionScope {
                        entry_node: entry.clone(),
                        max_parallel: iteration.body.max_parallel,
                        failure: iteration.source.body.failure.clone(),
                    },
                );
                candidates.push(WorkflowOutputRef {
                    step_id: exit.clone(),
                    pointer: String::new(),
                });
                instances.push(WorkflowIterationInstance {
                    index: item.index,
                    entry,
                    exit,
                });
            }
            let mut dependencies: BTreeSet<_> =
                instances.iter().map(|item| item.exit.clone()).collect();
            dependencies.insert(iteration.capture_node.clone());
            graph
                .nodes
                .to_mut()
                .get_mut(id)
                .ok_or(Error::Forbidden)?
                .dependencies = dependencies.clone();
            let control = graph
                .definition
                .to_mut()
                .controls
                .get_mut(id)
                .ok_or(Error::Forbidden)?;
            control.dependencies = dependencies.into_iter().collect();
            control.operation = WorkflowControlOperation::Collect { candidates };
            graph.iteration_instances.insert(id.clone(), instances);
        }
        Ok(graph)
    }
}
