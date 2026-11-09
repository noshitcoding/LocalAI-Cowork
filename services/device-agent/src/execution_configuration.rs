//! Frozen actor settings narrow the independently configured device owner policy.
use super::*;
use cowork_contracts::{AccessMode, AgentExecutionMode, Budgets, EffectiveAgentConfig};

#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Requested {
    agent_config: Option<EffectiveAgentConfig>,
    access_mode: Option<AccessMode>,
    budgets: Option<Budgets>,
}
#[derive(Clone)]
pub(super) struct RunSettings {
    pub agent: EffectiveAgentConfig,
    pub access_mode: AccessMode,
    pub budgets: Budgets,
    pub explicit: bool,
    pub deadline: Option<tokio::time::Instant>,
    pub frozen_context: Value,
}
tokio::task_local! {static SETTINGS:RunSettings;}

#[cfg(test)]
mod local_authority_tests {
    use super::*;
    #[test]
    fn remote_run_configuration_cannot_enable_native_rights_or_vm_launch() {
        for field in [
            "native_owner_rights",
            "local_execution_mode",
            "windows_sandbox",
            "programs",
        ] {
            let mut input = json!({"run_configuration": {}});
            input["run_configuration"][field] = json!(true);
            assert!(RunSettings::parse(&input, 8).is_err(), "{field}");
        }
    }
}
pub(super) async fn with_settings<F: std::future::Future>(
    settings: RunSettings,
    future: F,
) -> F::Output {
    SETTINGS.scope(settings, future).await
}
pub(super) fn current(engine: &ExecutionEngine) -> RunSettings {
    SETTINGS
        .try_with(Clone::clone)
        .unwrap_or_else(|_| RunSettings::defaults(engine.config.max_steps))
}
pub(super) fn check_deadline() -> Result<()> {
    ensure!(
        SETTINGS
            .try_with(|settings| settings
                .deadline
                .is_none_or(|deadline| tokio::time::Instant::now() < deadline))
            .unwrap_or(true),
        "device_run_time_budget_exceeded"
    );
    Ok(())
}

impl RunSettings {
    fn defaults(owner_steps: u16) -> Self {
        let budgets = Budgets {
            max_steps: u32::from(owner_steps),
            ..Budgets::default()
        };
        Self {
            agent: Default::default(),
            access_mode: AccessMode::ConfirmMutations,
            budgets,
            explicit: false,
            deadline: None,
            frozen_context: Value::Null,
        }
    }
    pub(super) fn parse(input: &Value, owner_steps: u16) -> Result<Self> {
        let Some(raw) = input.get("run_configuration").filter(|v| !v.is_null()) else {
            let mut defaults = Self::defaults(owner_steps);
            defaults.frozen_context = input
                .get("frozen_runtime_context")
                .cloned()
                .unwrap_or(Value::Null);
            defaults.freeze_skill_mode(
                input["prompt"]
                    .as_str()
                    .or_else(|| input["task"].as_str())
                    .unwrap_or(""),
            )?;
            return Ok(defaults);
        };
        ensure!(
            raw.get("budgets")
                .filter(|value| !value.is_null())
                .is_none_or(
                    |value| value
                        .as_object()
                        .is_some_and(|budgets| budgets.keys().all(|name| [
                            "max_steps",
                            "max_seconds",
                            "max_output_tokens",
                            "max_total_tokens"
                        ]
                        .contains(&name.as_str())))
                ),
            "device_run_configuration_invalid"
        );
        let request: Requested =
            serde_json::from_value(raw.clone()).context("device_run_configuration_invalid")?;
        let agent = request.agent_config.unwrap_or_default();
        let requested_budgets = request.budgets.unwrap_or_default();
        cowork_domain::validate_agent_values(&agent)?;
        cowork_domain::validate_budgets(&requested_budgets)?;
        let caps = Self::defaults(owner_steps).budgets;
        let budgets = Budgets {
            max_steps: requested_budgets.max_steps.min(caps.max_steps).min(
                agent
                    .behavior
                    .as_ref()
                    .map_or(u32::MAX, |b| b.max_tool_calls_per_loop),
            ),
            max_seconds: requested_budgets.max_seconds.min(caps.max_seconds),
            max_output_tokens: requested_budgets
                .max_output_tokens
                .min(caps.max_output_tokens),
            max_total_tokens: requested_budgets
                .max_total_tokens
                .min(caps.max_total_tokens),
        };
        let requested_access = request.access_mode.unwrap_or(AccessMode::ConfirmMutations);
        let access_mode = agent
            .access_mode
            .as_ref()
            .map(|mode| cowork_domain::narrow_access_mode(&requested_access, mode))
            .unwrap_or(requested_access);
        let mut value = Self {
            agent,
            access_mode,
            budgets,
            explicit: true,
            deadline: None,
            frozen_context: input
                .get("frozen_runtime_context")
                .cloned()
                .unwrap_or(Value::Null),
        };
        value.context(&value.frozen_context)?;
        value.freeze_skill_mode(
            input["prompt"]
                .as_str()
                .or_else(|| input["task"].as_str())
                .unwrap_or(""),
        )?;
        Ok(value)
    }
    fn mapped(name: &str) -> &str {
        match name {
            "list_files" => "list_workspace_files",
            "read_text_file" => "read_workspace_file",
            "write_text_file" => "write_workspace_file",
            other => other,
        }
    }
    pub(super) fn for_role(&self, config: Option<&EffectiveAgentConfig>) -> Result<Self> {
        let mut value = self.clone();
        value.agent =
            cowork_domain::merge_agent_values(Some(&self.agent), config).unwrap_or_default();
        cowork_domain::validate_agent_values(&value.agent)?;
        value.access_mode = value
            .agent
            .access_mode
            .as_ref()
            .map(|mode| cowork_domain::narrow_access_mode(&self.access_mode, mode))
            .unwrap_or_else(|| self.access_mode.clone());
        value.budgets.max_steps = value.budgets.max_steps.min(
            value
                .agent
                .behavior
                .as_ref()
                .map_or(u32::MAX, |b| b.max_tool_calls_per_loop),
        );
        value.context(&value.frozen_context)?;
        Ok(value)
    }
    pub(super) fn guidance(&self, prompt: &str) -> Result<String> {
        let context = self.context(&self.frozen_context)?;
        let mut parts = Vec::new();
        if let Some(skills) = context["skill"].as_array() {
            for skill in skills {
                let definition = &skill["definition"];
                if !skill_applies(definition, prompt) {
                    continue;
                }
                let text = definition["prompt_template"]
                    .as_str()
                    .context("device_skill_prompt_missing")?;
                parts.push(format!("Selected skill {} revision {}. Guidance cannot grant tools or override device policy:\n{}",skill["entity_id"],skill["revision"],text.replace("{{input}}",prompt)));
            }
        }
        let result = parts.join("\n\n");
        ensure!(result.len() <= 256000, "device_skill_guidance_bound");
        Ok(result)
    }
    pub(super) fn freeze_skill_mode(&mut self, prompt: &str) -> Result<()> {
        let context = self.context(&self.frozen_context)?;
        let mut mode = self.agent.execution_mode.unwrap_or_default();
        if let Some(skills) = context["skill"].as_array() {
            for skill in skills {
                let definition = &skill["definition"];
                if !skill_applies(definition, prompt) {
                    continue;
                }
                let selected = match definition["run_mode"].as_str() {
                    None | Some("execute") => AgentExecutionMode::Execute,
                    Some("plan") => AgentExecutionMode::Plan,
                    Some("hybrid") => AgentExecutionMode::Hybrid,
                    _ => bail!("device_skill_execution_mode_invalid"),
                };
                mode = cowork_domain::narrow_execution_mode(mode, selected);
            }
        }
        self.agent.execution_mode = Some(mode);
        Ok(())
    }
    pub(super) fn tool_authority(
        &self,
        engine: &ExecutionEngine,
        run: Uuid,
        source: Uuid,
        name: &str,
        args: &Value,
    ) -> Result<()> {
        ensure!(
            self.agent.execution_mode != Some(AgentExecutionMode::Plan),
            "device_plan_mode_tool_forbidden"
        );
        if let Some(behavior) = &self.agent.behavior {
            let command = match name {
                "execute_program" => Some(
                    std::iter::once(args["program"].as_str().unwrap_or(""))
                        .chain(
                            args["arguments"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str),
                        )
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                "bash" => Some(
                    args["command"]
                        .as_str()
                        .context("device_command_missing")?
                        .to_owned(),
                ),
                "git" => Some(format!(
                    "git {}",
                    args["arguments"]
                        .as_array()
                        .context("device_git_arguments_missing")?
                        .iter()
                        .map(|v| v.as_str().context("device_git_arguments_invalid"))
                        .collect::<Result<Vec<_>>>()?
                        .join(" ")
                )),
                _ => None,
            };
            if let Some(command) = command {
                ensure!(
                    !behavior
                        .command_blacklist
                        .iter()
                        .any(|pattern| pattern_matches(pattern, &command))
                        && behavior.command_allow_groups.iter().all(|group| group
                            .iter()
                            .any(|pattern| pattern_matches(pattern, &command))),
                    "device_command_denied_by_frozen_agent"
                );
            }
        }
        let prefix = format!("{run}-");
        let mut calls = 0u32;
        let mut rows = 0usize;
        for entry in fs::read_dir(&engine.journal_directory)? {
            let path = entry?.path();
            if !path.file_name().is_some_and(|p| {
                p.to_string_lossy().starts_with(&prefix) && p.to_string_lossy().ends_with(".json")
            }) {
                continue;
            }
            rows += 1;
            ensure!(rows <= 4096, "device_original_journal_bound");
            let value: Value =
                serde_json::from_reader(crate::config::private_file(&path, 4 * 1024 * 1024)?)?;
            if value["phase"] == "configured_tool_invoked" {
                calls = calls
                    .checked_add(1)
                    .context("device_tool_budget_exceeded")?;
            }
        }
        ensure!(
            calls < self.budgets.max_steps,
            "device_tool_budget_exceeded"
        );
        engine.admit(run,&format!("configured-tool/{source}"),json!({"phase":"configured_tool_invoked","tool":name,"arguments_sha256":digest(&serde_json::to_vec(args)?)}))
    }
    pub(super) fn context(&self, context: &Value) -> Result<Value> {
        let mut context = context.clone();
        ensure!(
            context.is_null()
                || context.is_object()
                || self.agent.skill_ids.is_none() && self.agent.memory_ids.is_none(),
            "device_versioned_library_context_not_supported"
        );
        if let Some(object) = context.as_object_mut() {
            for (kind, selected) in [
                ("skill", self.agent.skill_ids.as_ref()),
                ("memory", self.agent.memory_ids.as_ref()),
            ] {
                let Some(selected) = selected else {
                    continue;
                };
                let entries = object
                    .get(kind)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for id in selected {
                    ensure!(
                        entries
                            .iter()
                            .any(|entry| entry["entity_id"].as_str() == Some(id)
                                && entry["revision"].as_i64().is_some_and(|r| r > 0)
                                && entry["etag"].is_string()
                                && entry["definition"].is_object()),
                        "device_selected_context_not_in_original_snapshot: {kind}/{id}"
                    );
                }
                if selected.is_empty() {
                    object.remove(kind);
                } else {
                    object.insert(
                        kind.into(),
                        json!(
                            entries
                                .into_iter()
                                .filter(|entry| selected
                                    .iter()
                                    .any(|id| entry["entity_id"].as_str() == Some(id)))
                                .collect::<Vec<_>>()
                        ),
                    );
                }
            }
        } else if self
            .agent
            .skill_ids
            .as_ref()
            .is_some_and(|ids| !ids.is_empty())
            || self
                .agent
                .memory_ids
                .as_ref()
                .is_some_and(|ids| !ids.is_empty())
        {
            bail!("device_selected_context_not_in_original_snapshot");
        }
        Ok(context)
    }
    pub(super) fn tools(&self, mut tools: Vec<Value>) -> Result<Vec<Value>> {
        if let Some(filter) = &self.agent.tool_filter {
            for name in filter {
                ensure!(
                    tools
                        .iter()
                        .any(|tool| tool["function"]["name"].as_str() == Some(Self::mapped(name))),
                    "device_agent_tool_filter_not_locally_available: {name}"
                );
            }
            tools.retain(|tool| {
                filter
                    .iter()
                    .any(|name| tool["function"]["name"].as_str() == Some(Self::mapped(name)))
            });
        }
        if matches!(self.access_mode, AccessMode::ReadOnly) {
            tools.retain(|tool| {
                matches!(
                    tool["function"]["name"].as_str(),
                    Some(
                        "list_workspace_files"
                            | "read_workspace_file"
                            | "list_sources"
                            | "request_input"
                            | "read_file"
                            | "glob"
                            | "grep"
                    )
                )
            });
        }
        if self.agent.execution_mode == Some(AgentExecutionMode::Plan) {
            tools.clear();
        }
        Ok(tools)
    }
    pub(super) fn apply(&self, body: &mut Value, ollama: bool) -> Result<u64> {
        ensure!(body.is_object(), "device_model_request_invalid");
        let maximum = body
            .get("max_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(u64::from(self.budgets.max_output_tokens))
            .min(u64::from(self.budgets.max_output_tokens));
        ensure!(maximum > 0, "device_model_output_budget_invalid");
        body["max_tokens"] = json!(maximum);
        if let Some(temperature) = self.agent.generation.temperature {
            body["temperature"] = json!(temperature);
        }
        // UTF-8 wire bytes plus a fixed template allowance conservatively
        // reserve context. Unreported usage never creates more spending room.
        let reserve = (serde_json::to_vec(body)?.len() as u64)
            .checked_add(4096)
            .and_then(|n| n.checked_add(maximum))
            .context("device_model_token_budget_exceeded")?;
        if ollama {
            let object = body.as_object_mut().unwrap();
            object.remove("max_tokens");
            let temperature = object.remove("temperature");
            let options = object.entry("options").or_insert_with(|| json!({}));
            ensure!(options.is_object(), "device_model_options_invalid");
            options["num_predict"] = json!(maximum);
            if let Some(temperature) = temperature {
                options["temperature"] = temperature;
            }
        }
        Ok(reserve)
    }
    pub(super) fn reserve(&self, engine: &ExecutionEngine, run: Uuid, tokens: u64) -> Result<()> {
        let (previous, _) = Self::reservation_totals(engine, run)?;
        ensure!(
            previous
                .checked_add(tokens)
                .is_some_and(|total| total <= self.budgets.max_total_tokens),
            "device_model_token_budget_exceeded"
        );
        Ok(())
    }
    pub(super) fn reservation_totals(engine: &ExecutionEngine, run: Uuid) -> Result<(u64, bool)> {
        let prefix = format!("{run}-");
        let mut previous = 0u64;
        let mut sampled = false;
        let mut count = 0usize;
        for entry in fs::read_dir(&engine.journal_directory)? {
            let path = entry?.path();
            if !path.file_name().is_some_and(|n| {
                n.to_string_lossy().starts_with(&prefix) && n.to_string_lossy().ends_with(".json")
            }) {
                continue;
            }
            count += 1;
            ensure!(count <= 4096, "device_original_journal_bound");
            let value: Value =
                serde_json::from_reader(crate::config::private_file(&path, 4 * 1024 * 1024)?)?;
            if value["phase"] == "model_dispatched" {
                sampled |= value["scope"] == "mcp-task-sampling";
                previous = previous
                    .checked_add(
                        value["reserved_tokens"]
                            .as_u64()
                            .context("device_prior_model_budget_unavailable_no_replay")?,
                    )
                    .context("device_model_token_budget_exceeded")?;
            }
        }
        Ok((previous, sampled))
    }
}
fn pattern_matches(pattern: &str, text: &str) -> bool {
    let mut cursor = 0usize;
    let parts = pattern.split('*').collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let remaining = &text[cursor..];
        if index == 0 && !pattern.starts_with('*') {
            if !remaining.starts_with(part) {
                return false;
            }
            cursor += part.len();
        } else if index == parts.len() - 1 && !pattern.ends_with('*') {
            return remaining.ends_with(part);
        } else if let Some(offset) = remaining.find(part) {
            cursor += offset + part.len();
        } else {
            return false;
        }
    }
    pattern.ends_with('*') || cursor == text.len()
}
fn skill_applies(definition: &Value, prompt: &str) -> bool {
    definition["enabled"] != false
        && definition["trigger_pattern"]
            .as_str()
            .is_none_or(|pattern| pattern_matches(&pattern.to_lowercase(), &prompt.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> Value {
        json!({"run_configuration":{"agent_config":{"instructions":"Copied personality","generation":{"temperature":0.0,"timeout_ms":1000}},"access_mode":"read_only","budgets":{"max_steps":64,"max_seconds":86400,"max_output_tokens":9999,"max_total_tokens":1000000}}})
    }
    #[test]
    fn authenticated_copies_filter_role_context_and_skill_modes_without_db_fallback() {
        let skill = Uuid::new_v4().to_string();
        let memory = Uuid::new_v4().to_string();
        let other = Uuid::new_v4().to_string();
        let mut input = input();
        input["prompt"] = json!("Research this");
        input["run_configuration"]["agent_config"]["skill_ids"] = json!([skill]);
        input["run_configuration"]["agent_config"]["memory_ids"] = json!([memory]);
        input["frozen_runtime_context"] = json!({"skill":[{"entity_id":skill,"revision":3,"etag":"skill:3","definition":{"name":"Plan","prompt_template":"Inspect {{input}}","trigger_pattern":"research*","run_mode":"plan"}}],"memory":[{"entity_id":memory,"revision":2,"etag":"memory:2","definition":{"content":"Selected original"}},{"entity_id":other,"revision":1,"etag":"other:1","definition":{"content":"Other original"}}]});
        let settings = RunSettings::parse(&input, 8).unwrap();
        let context = settings.context(&settings.frozen_context).unwrap();
        assert_eq!(context["memory"].as_array().unwrap().len(), 1);
        assert_eq!(context["memory"][0]["revision"], 2);
        assert_eq!(
            settings.agent.execution_mode,
            Some(AgentExecutionMode::Plan)
        );
        assert!(
            settings
                .guidance("Research this")
                .unwrap()
                .contains("Inspect Research this")
        );
        assert!(settings.guidance("Other task").unwrap().is_empty());
        assert!(
            settings
                .tools(vec![definition(
                    "read_workspace_file",
                    "Read",
                    json!({}),
                    &[]
                )])
                .unwrap()
                .is_empty()
        );
        let mut changed = input;
        changed["run_configuration"]["agent_config"]["memory_ids"] = json!([Uuid::new_v4()]);
        assert!(RunSettings::parse(&changed, 8).is_err());
    }
    #[test]
    fn frozen_settings_validate_and_only_narrow_owner_ceilings() {
        let settings = RunSettings::parse(&input(), 8).unwrap();
        assert_eq!(settings.budgets.max_steps, 8);
        assert_eq!(settings.budgets.max_seconds, 1800);
        assert_eq!(settings.budgets.max_output_tokens, 4096);
        assert_eq!(settings.budgets.max_total_tokens, 262144);
        assert_eq!(settings.agent.instructions, "Copied personality");
        let mut invalid = input();
        invalid["run_configuration"]["api_key"] = json!("forbidden");
        assert!(RunSettings::parse(&invalid, 8).is_err());
        invalid = input();
        invalid["run_configuration"]["agent_config"]["generation"]["temperature"] = json!(3);
        assert!(RunSettings::parse(&invalid, 8).is_err());
        let partial = RunSettings::parse(
            &json!({"run_configuration":{"agent_config":{"instructions":"Only instructions"}}}),
            8,
        )
        .unwrap();
        assert_eq!(partial.agent.instructions, "Only instructions");
        assert_eq!(partial.budgets.max_steps, 8);
        let mut selected = input();
        selected["run_configuration"]["agent_config"]["skill_ids"] = json!([Uuid::new_v4()]);
        assert!(RunSettings::parse(&selected, 8).is_err());
        selected["run_configuration"]["agent_config"]["skill_ids"] = json!([]);
        let empty = RunSettings::parse(&selected, 8).unwrap();
        let narrowed = empty
            .context(&json!({"skill":[{"entity_id":"not-authority"}],"memory":[]}))
            .unwrap();
        assert!(narrowed.get("skill").is_none());
        assert!(narrowed.get("memory").is_some());
    }
    #[tokio::test]
    async fn expired_scope_forbids_new_dispatches_without_waiting_for_outer_timer_poll() {
        let mut settings = RunSettings::parse(&input(), 8).unwrap();
        settings.deadline = Some(tokio::time::Instant::now() - Duration::from_millis(1));
        assert!(
            with_settings(settings, async { check_deadline() })
                .await
                .is_err()
        );
    }
    #[test]
    fn filters_map_exact_file_equivalents_and_never_expand_mcp_or_readonly() {
        let mut settings = RunSettings::parse(&input(), 8).unwrap();
        let tools = vec![
            definition("read_workspace_file", "Read", json!({}), &[]),
            definition("write_workspace_file", "Write", json!({}), &[]),
        ];
        settings.agent.tool_filter = Some(vec!["read_text_file".into(), "write_text_file".into()]);
        let selected = settings.tools(tools.clone()).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0]["function"]["name"], "read_workspace_file");
        settings.agent.tool_filter = Some(vec!["mcp_foreign_server_alias".into()]);
        assert!(settings.tools(tools).is_err());
        assert!(
            crew::reject_unsupported_configuration(
                &json!({"agents":[{"id":"role","agent_config":{"generation":{"temperature":0.0}}}]})
            )
            .is_ok()
        );
        assert!(
            crew::reject_unsupported_configuration(
                &json!({"agents":[{"id":"role","generation":{"temperature":0.0}}]})
            )
            .is_err()
        );
        assert!(
            crew::reject_unsupported_configuration(
                &json!({"agents":[{"id":"role","backstory":"Existing original role text"}]})
            )
            .is_ok()
        );
    }
    #[tokio::test]
    async fn configured_sampling_requires_original_model_and_preserves_role_settings() {
        let settings = RunSettings::parse(&input(), 8).unwrap();
        with_settings(settings, async {
            assert!(!mcp::interactions::sampling_available());
            mcp::interactions::with_model(
                "device-owner-model".into(),
                vec![json!({"role":"system","content":"Frozen role context"})],
                async {
                    assert!(mcp::interactions::sampling_available());
                    let mut body = json!({"model":"device-owner-model","messages":[]});
                    SETTINGS
                        .with(|settings| settings.apply(&mut body, false))
                        .unwrap();
                    assert_eq!(body["temperature"], 0.0);
                    assert_eq!(body["max_tokens"], 4096);
                },
            )
            .await;
            assert!(!mcp::interactions::sampling_available());
        })
        .await;
    }
    #[test]
    fn provider_wire_receives_zero_temperature_and_bounded_output_for_both_apis() {
        let settings = RunSettings::parse(&input(), 8).unwrap();
        let mut body = json!({"model":"device-only","messages":[{"role":"user","content":"test"}]});
        let reserved = settings.apply(&mut body, false).unwrap();
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["max_tokens"], 4096);
        assert!(reserved > 8192);
        let mut body = json!({"model":"device-only","messages":[],"max_tokens":100});
        settings.apply(&mut body, true).unwrap();
        assert_eq!(body["options"]["temperature"], 0.0);
        assert_eq!(body["options"]["num_predict"], 100);
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("temperature").is_none());
    }

    struct EventsOnly;
    #[async_trait]
    impl DeviceControl for EventsOnly {
        async fn run_json(
            &self,
            _run: Uuid,
            _lease: Uuid,
            _method: &str,
            suffix: &str,
            _body: Value,
        ) -> Result<Value> {
            ensure!(suffix == "events", "unexpected effect");
            Ok(json!({}))
        }
        async fn get_chunk(&self, _run: Uuid, _lease: Uuid, _digest: &str) -> Result<Vec<u8>> {
            bail!("unexpected read")
        }
        async fn put_chunk(
            &self,
            _run: Uuid,
            _lease: Uuid,
            _manifest: Uuid,
            _digest: &str,
            _bytes: Vec<u8>,
        ) -> Result<Value> {
            bail!("unexpected write")
        }
        async fn artifact(
            &self,
            _run: Uuid,
            _lease: Uuid,
            _path: &str,
            _source: Uuid,
            _bytes: Vec<u8>,
        ) -> Result<Value> {
            bail!("unexpected artifact")
        }
    }
    #[tokio::test]
    async fn original_device_wire_applies_frozen_instructions_and_never_replays() {
        use axum::{Json, Router, extract::State, routing::post};
        use std::sync::Mutex;
        type Captured = Arc<Mutex<Vec<Value>>>;
        async fn reply(State(captured): State<Captured>, Json(body): Json<Value>) -> Json<Value> {
            let slow = body["messages"]
                .as_array()
                .and_then(|messages| messages.last())
                .is_some_and(|message| message["content"] == "Wait beyond timeout");
            let content = if body.get("response_format").is_some() {
                "{\"answer\":\"done\"}"
            } else {
                "Completed once"
            };
            captured.lock().unwrap().push(body);
            if slow {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Json(json!({"choices":[{"message":{"content":content}}]}))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let captured: Captured = Default::default();
        let app = Router::new()
            .route("/chat/completions", post(reply))
            .with_state(captured.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let directory = tempfile::tempdir().unwrap();
        let private_root = directory.path().join("private-device");
        crate::new_directory(&private_root).unwrap();
        let engine = ExecutionEngine::open(ExecutionConfig {
            data_directory: private_root,
            model: Some(ModelConfiguration {
                base_url: endpoint,
                api_key: None,
                model: "device-owner-model".into(),
                api: "openai".into(),
                implementation: "openai_compatible".into(),
                timeout_seconds: 10,
            }),
            programs: Default::default(),
            environment: Default::default(),
            github_token: None,
            mcp_bindings: vec![],
            allowed_tools: vec![],
            native_owner_rights: false,
            require_local_consent: true,
            max_steps: 8,
            interop: Default::default(),
            capability_names: vec![],
        })
        .unwrap();
        let run = Uuid::new_v4();
        let lease = Uuid::new_v4();
        let stop = CancellationToken::new();
        let mut original = input();
        original["prompt"] = json!("Run once");
        let settings = RunSettings::parse(&original, 8).unwrap();
        let result = with_settings(
            settings.clone(),
            engine.execute_model(&EventsOnly, run, lease, &original, &stop),
        )
        .await
        .unwrap();
        assert_eq!(result["content"], "Completed once");
        assert_eq!(result["usage_complete"], false);
        assert!(result["usage"]["total_tokens"].is_null());
        assert!(result["reserved_tokens"].as_u64().unwrap() > 0);
        assert!(
            with_settings(
                settings,
                engine.execute_model(&EventsOnly, run, lease, &original, &stop)
            )
            .await
            .is_err()
        );
        let slow_run = Uuid::new_v4();
        let mut slow = original.clone();
        slow["prompt"] = json!("Wait beyond timeout");
        let settings = RunSettings::parse(&slow, 8).unwrap();
        assert!(
            with_settings(
                settings.clone(),
                engine.execute_model(&EventsOnly, slow_run, lease, &slow, &stop)
            )
            .await
            .is_err()
        );
        assert!(
            with_settings(
                settings,
                engine.execute_model(&EventsOnly, slow_run, lease, &slow, &stop)
            )
            .await
            .is_err()
        );
        {
            let calls = captured.lock().unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0]["model"], "device-owner-model");
            assert_eq!(calls[0]["messages"][1]["content"], "Copied personality");
            assert_eq!(calls[0]["temperature"], 0.0);
            assert_eq!(calls[0]["max_tokens"], 4096);
        }
        let mut hybrid = original.clone();
        hybrid["prompt"] = json!("Plan then execute");
        hybrid["run_configuration"]["agent_config"]["execution_mode"] = json!("hybrid");
        hybrid["output"] = json!({"name":"answer","schema":{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}});
        let hybrid_run = Uuid::new_v4();
        let settings = RunSettings::parse(&hybrid, 8).unwrap();
        let result = with_settings(
            settings,
            engine.execute_model(&EventsOnly, hybrid_run, lease, &hybrid, &stop),
        )
        .await
        .unwrap();
        assert_eq!(result["steps"], 2);
        assert_eq!(result["structured_output"]["answer"], "done");
        {
            let calls = captured.lock().unwrap();
            assert_eq!(calls.len(), 4);
            assert!(calls[2].get("response_format").is_none());
            assert!(calls[2].get("tools").is_none());
            assert!(
                calls[2]["messages"].as_array().unwrap().last().unwrap()["content"]
                    .as_str()
                    .unwrap()
                    .contains("Planning phase")
            );
            assert_eq!(calls[3]["response_format"]["type"], "json_schema");
            assert!(
                calls[3]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| message["role"] == "assistant"
                        && message["content"] == "Completed once")
            );
        }
        // Crew callbacks use this same dispatch boundary under a narrowed role
        // scope, rather than bypassing the copied generation and owner caps.
        let parent = RunSettings::parse(&original, 8).unwrap();
        let role: EffectiveAgentConfig = serde_json::from_value(json!({"instructions":"Crew role copy","generation":{"temperature":0.25,"timeout_ms":1000},"access_mode":"autonomous","tool_filter":["read_text_file"],"behavior":{"command_allow_groups":[["git status*"]],"command_blacklist":["git status --*"] ,"max_tool_calls_per_loop":1,"fallback_to_human_on_repeated_failure":true}})).unwrap();
        let role = parent.for_role(Some(&role)).unwrap();
        assert_eq!(role.access_mode, AccessMode::ReadOnly);
        assert_eq!(role.budgets.max_steps, 1);
        let role_run = Uuid::new_v4();
        let response = with_settings(role.clone(), device_model::turn(&engine, &EventsOnly, role_run, lease, "crew/role/model/0", json!({"model":"device-owner-model","messages":[{"role":"system","content":role.agent.instructions},{"role":"user","content":"Role request"}]}), &stop)).await.unwrap();
        assert_eq!(
            response["choices"][0]["message"]["content"],
            "Completed once"
        );
        {
            let calls = captured.lock().unwrap();
            assert_eq!(calls.len(), 5);
            assert_eq!(calls[4]["temperature"], 0.25);
            assert_eq!(calls[4]["messages"][0]["content"], role.agent.instructions);
        }
        assert!(
            with_settings(
                role.clone(),
                device_model::turn(
                    &engine,
                    &EventsOnly,
                    role_run,
                    lease,
                    "crew/role/model/1",
                    json!({"model":"device-owner-model","messages":[]}),
                    &stop
                )
            )
            .await
            .is_err()
        );
        assert_eq!(captured.lock().unwrap().len(), 5);
        let tool_run = Uuid::new_v4();
        assert!(
            role.tool_authority(
                &engine,
                tool_run,
                Uuid::new_v4(),
                "git",
                &json!({"arguments":["status","--porcelain"]})
            )
            .is_err()
        );
        assert!(
            role.tool_authority(
                &engine,
                tool_run,
                Uuid::new_v4(),
                "git",
                &json!({"arguments":["push"]})
            )
            .is_err()
        );
        let source = Uuid::new_v4();
        role.tool_authority(
            &engine,
            tool_run,
            source,
            "git",
            &json!({"arguments":["status"]}),
        )
        .unwrap();
        assert!(
            role.tool_authority(
                &engine,
                tool_run,
                source,
                "git",
                &json!({"arguments":["status"]})
            )
            .is_err()
        );
        assert!(
            role.tool_authority(
                &engine,
                tool_run,
                Uuid::new_v4(),
                "git",
                &json!({"arguments":["status"]})
            )
            .is_err()
        );
        let mut office = original.clone();
        office["windows_office"] = json!({"action":"create_document","path":"unapproved.docx"});
        let office_run = Uuid::new_v4();
        let error = engine.execute(&EventsOnly,json!({"lease_token":lease,"run":{"spec":{"id":office_run,"project_id":Uuid::new_v4(),"snapshot_id":null,"input":office}}}),&stop).await.err().unwrap();
        assert!(
            error
                .to_string()
                .contains("device_direct_office_denied_by_frozen_agent")
        );
        assert_eq!(captured.lock().unwrap().len(), 5);
        let mut generic = original.clone();
        generic.as_object_mut().unwrap().remove("prompt");
        generic["run_configuration"]["agent_config"]["memory_ids"] = json!([]);
        generic["frozen_runtime_context"] = json!({"memory":[{"entity_id":Uuid::new_v4(),"revision":1,"etag":"original:1","definition":{"content":"DO_NOT_LEAK_EXCLUDED_MEMORY"}}]});
        let generic_settings = RunSettings::parse(&generic, 8).unwrap();
        with_settings(
            generic_settings,
            engine.execute_model(&EventsOnly, Uuid::new_v4(), lease, &generic, &stop),
        )
        .await
        .unwrap();
        {
            let calls = captured.lock().unwrap();
            assert_eq!(calls.len(), 6);
            assert!(
                !calls[5]["messages"]
                    .to_string()
                    .contains("DO_NOT_LEAK_EXCLUDED_MEMORY")
            );
        }
        engine.shutdown();
        server.abort();
        let _ = server.await;
    }
}
