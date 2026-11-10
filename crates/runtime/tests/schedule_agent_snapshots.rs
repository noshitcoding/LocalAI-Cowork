use cowork_contracts::*;
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::library_config::LibraryOwner;
use cowork_storage::{Store, WriteSchedule};
use serde_json::json;

#[tokio::test]
async fn model_and_workflow_occurrences_keep_the_copied_agent_after_library_edit() {
    for workflow in [false, true] {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Scheduled agent".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Copied scheduled config".into(),
            })
            .await
            .unwrap();
        let profile:ProviderProfile=serde_json::from_value(json!({"id":"fixture","name":"Fixture","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true})).unwrap();
        store.insert_provider(&profile).await.unwrap();
        let mut write_agent = WriteAgentTemplate {
            idempotency_key: "initial-agent".into(),
            expected_revision: 0,
            value: AgentTemplateDefinition {
                name: "Scheduled reviewer".into(),
                configuration: AgentConfiguration {
                    instructions: "COPIED_SCHEDULE_INSTRUCTIONS".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
        };
        let agent = store
            .library_write_agent_template(LibraryOwner::Local, None, &write_agent)
            .await
            .unwrap();
        let copied = EffectiveAgentConfig {
            instructions: write_agent.value.configuration.instructions.clone(),
            generation: GenerationSettings {
                temperature: Some(0.0),
                timeout_ms: Some(4000),
            },
            agent_source: Some(LibrarySource {
                id: agent.id.clone(),
                revision: agent.revision,
            }),
            tool_filter: Some(vec![]),
            ..Default::default()
        };
        let mut input:WriteSchedule=serde_json::from_value(json!({"id":null,"expected_revision":0,"name":"Copied schedule","timezone":"UTC","recurrence":{"kind":"interval","seconds":60},"template":{"agent_config":copied,"order_id":order.id,"prompt":"Review supplied evidence","provider_id":"fixture","provider_revision":1},"paused":false})).unwrap();
        if workflow {
            let rights = json!({"tools":[],"folders":[]});
            let budgets = Budgets::default();
            let definition=serde_json::from_value(json!({"format_version":1,"name":"Scheduled workflow","mode":{"mode":"sequential"},"failure":"stop","rights":rights,"budgets":budgets,"roles":[{"id":"Review","instructions":"Review supplied evidence","provider_id":"fixture","provider_revision":1,"rights":rights,"budgets":budgets,"delegate_to":[]}],"steps":[{"id":"review","role_id":"Review","goal":"Review evidence","dependencies":[],"rights":rights,"budgets":budgets}]})).unwrap();
            let library = store
                .create_workflow(&project.id, "workflow", &definition)
                .await
                .unwrap();
            input.template.workflow=Some(serde_json::from_value(json!({"kind":"workflow","agent_config":{"instructions":"nested old snapshot","generation":{}},"order_id":order.id,"base_revision":order.revision,"idempotency_key":"not-an-occurrence","prompt":"old prompt","workflow_id":library.id,"version_id":library.versions[0].id,"access_mode":"read_only","budgets":budgets})).unwrap());
        }
        let saved = store
            .productivity_write_schedule_idempotent(&project.id, input, "save-schedule")
            .await
            .unwrap();
        let claimed = store
            .productivity_claim_schedule(
                Some(&project.id),
                Some(&saved.id),
                true,
                Some(saved.revision),
            )
            .await
            .unwrap()
            .unwrap();
        let occurrence = claimed.active_occurrence.unwrap();
        assert_eq!(
            serde_json::to_value(&occurrence.request.agent_config).unwrap(),
            serde_json::to_value(Some(&copied)).unwrap()
        );
        write_agent.idempotency_key = "edited-agent".into();
        write_agent.expected_revision = agent.revision;
        write_agent.value.configuration.instructions = "MUTATED_LIBRARY_INSTRUCTIONS".into();
        store
            .library_write_agent_template(LibraryOwner::Local, Some(&agent.id), &write_agent)
            .await
            .unwrap();
        let runtime = Runtime::new(
            store,
            vec![ConfiguredProvider {
                profile,
                credential_env: None,
            }],
            true,
        )
        .unwrap();
        let run = if let Some(request) = &occurrence.workflow_request {
            assert_eq!(
                serde_json::to_value(&request.agent_config).unwrap(),
                serde_json::to_value(Some(&copied)).unwrap()
            );
            runtime.create_product_workflow_run(request).await.unwrap()
        } else {
            runtime.create_run(&occurrence.request).await.unwrap()
        };
        let accepted = run.spec.agent_config.as_ref().unwrap();
        assert_eq!(accepted.instructions, copied.instructions);
        assert_eq!(accepted.generation.temperature, Some(0.0));
        assert_eq!(accepted.agent_source.as_ref().unwrap().revision, 1);
        if workflow {
            let child = runtime
                .store
                .workflow_node_spec(&project.id, &run.id, "review")
                .await
                .unwrap();
            assert!(
                child
                    .agent_config
                    .unwrap()
                    .instructions
                    .contains("COPIED_SCHEDULE_INSTRUCTIONS")
            );
        }
        let history = runtime
            .store
            .native_schedule_history(&project.id, &saved.id, None, 50)
            .await
            .unwrap();
        let original = history
            .items
            .iter()
            .find_map(|entry| entry.occurrence.as_ref())
            .unwrap();
        assert_eq!(original.id, occurrence.id);
        assert!(
            !serde_json::to_string(original)
                .unwrap()
                .contains("MUTATED_LIBRARY_INSTRUCTIONS")
        );
        assert!(
            runtime
                .store
                .productivity_schedules(&project.id)
                .await
                .unwrap()[0]
                .active_occurrence
                .is_some()
        );
    }
}
