use cowork_model_accounting::context::*;
use cowork_model_accounting::money::*;
use cowork_model_accounting::tokenizer::*;
use cowork_model_accounting::{Error, digest, sha256};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Barrier, Mutex};

fn tokenizer_fixture(
    context_tokens: u64,
) -> (
    Vec<u8>,
    Vec<u8>,
    TokenizerQualification,
    TrustedTokenizerBinding,
) {
    let vocabulary: BTreeMap<String, u32> = [
        ("[UNK]", 0),
        ("hello", 1),
        ("world", 2),
        ("你好", 3),
        ("system", 4),
        ("keep", 5),
        ("history", 6),
    ]
    .into_iter()
    .map(|(s, id)| (s.into(), id))
    .collect();
    let artifact = serde_json::to_vec(&json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],"normalizer":null,"pre_tokenizer":{"type":"Whitespace"},"post_processor":null,"decoder":null,"model":{"type":"WordLevel","vocab":vocabulary,"unk_token":"[UNK]"}})).unwrap();
    let template = b"fixture-whole-exchange-v1; generation prompt already included".to_vec();
    let model = ModelIdentity {
        provider_id: "synthetic-reference".into(),
        model_id: "fixture-wordlevel".into(),
        model_sha256: sha256(b"fixture-weights"),
        tokenizer_sha256: sha256(&artifact),
        vocabulary_sha256: digest(&vocabulary).unwrap(),
        chat_template_sha256: sha256(&template),
        context_tokens,
    };
    let qualification = TokenizerQualification {
        model: model.clone(),
        engine: ENGINE.into(),
        reference_implementation:
            "explicit synthetic WordLevel reference vectors; no real model qualification".into(),
        parity_evidence_sha256: sha256(b"independent explicit fixture vectors"),
        reference_encodings: vec![
            ReferenceEncoding {
                rendered_text: "hello world".into(),
                token_ids: vec![1, 2],
            },
            ReferenceEncoding {
                rendered_text: "你好".into(),
                token_ids: vec![3],
            },
            ReferenceEncoding {
                rendered_text: "".into(),
                token_ids: vec![],
            },
            ReferenceEncoding {
                rendered_text: "unknown".into(),
                token_ids: vec![0],
            },
        ],
    };
    let trusted = TrustedTokenizerBinding {
        model,
        qualification_sha256: digest(&qualification).unwrap(),
    };
    (artifact, template, qualification, trusted)
}

fn tokenizer(context: u64) -> QualifiedTokenizer {
    let (artifact, template, qualification, trusted) = tokenizer_fixture(context);
    QualifiedTokenizer::load(&artifact, &template, &qualification, &trusted).unwrap()
}

#[test]
fn actual_tokenizer_counts_unicode_empty_and_exact_context_boundary() {
    let tokenizer = tokenizer(4);
    let mut prompt = RenderedPrompt {
        model: tokenizer.model().clone(),
        logical_request_sha256: sha256(b"whole logical request"),
        text: "hello world 你好".into(),
    };
    let count = tokenizer.count(&prompt, 1).unwrap();
    assert_eq!(count.input_tokens, 3);
    assert_eq!(count.rendered_sha256, sha256(prompt.text.as_bytes()));
    assert_eq!(tokenizer.count(&prompt, 2), Err(Error::ContextLimit));
    prompt.text.clear();
    assert_eq!(tokenizer.count(&prompt, 4).unwrap().input_tokens, 0);
    assert_eq!(tokenizer.count(&prompt, u64::MAX), Err(Error::ContextLimit));
}

#[test]
fn tokenizer_rejects_wrong_weights_vocab_template_vectors_and_trusted_record() {
    let (artifact, template, qualification, trusted) = tokenizer_fixture(100);
    let mut bad = trusted.clone();
    bad.model.model_sha256 = sha256(b"another model");
    assert!(matches!(
        QualifiedTokenizer::load(&artifact, &template, &qualification, &bad),
        Err(Error::UnqualifiedTokenizer)
    ));
    assert!(matches!(
        QualifiedTokenizer::load(&artifact, b"wrong chat template", &qualification, &trusted),
        Err(Error::UnqualifiedTokenizer)
    ));
    let mut forged = qualification.clone();
    forged.reference_encodings[0].token_ids = vec![999];
    let trusted_forged = TrustedTokenizerBinding {
        model: forged.model.clone(),
        qualification_sha256: digest(&forged).unwrap(),
    };
    assert!(matches!(
        QualifiedTokenizer::load(&artifact, &template, &forged, &trusted_forged),
        Err(Error::UnqualifiedTokenizer)
    ));
    let mut bad_vocab = qualification.clone();
    bad_vocab.model.vocabulary_sha256 = sha256(b"wrong vocabulary");
    let trusted_bad = TrustedTokenizerBinding {
        model: bad_vocab.model.clone(),
        qualification_sha256: digest(&bad_vocab).unwrap(),
    };
    assert!(matches!(
        QualifiedTokenizer::load(&artifact, &template, &bad_vocab, &trusted_bad),
        Err(Error::UnqualifiedTokenizer)
    ));
    assert!(matches!(
        QualifiedTokenizer::load(b"{}", &template, &qualification, &trusted),
        Err(Error::UnqualifiedTokenizer)
    ));
}

#[test]
fn enabled_truncation_is_never_accepted_as_an_exact_prompt_count() {
    let (artifact, template, mut qualification, _) = tokenizer_fixture(100);
    let mut config: serde_json::Value = serde_json::from_slice(&artifact).unwrap();
    config["truncation"] =
        json!({"direction":"Right","max_length":1,"strategy":"LongestFirst","stride":0});
    let artifact = serde_json::to_vec(&config).unwrap();
    qualification.model.tokenizer_sha256 = sha256(&artifact);
    let trusted = TrustedTokenizerBinding {
        model: qualification.model.clone(),
        qualification_sha256: digest(&qualification).unwrap(),
    };
    assert!(matches!(
        QualifiedTokenizer::load(&artifact, &template, &qualification, &trusted),
        Err(Error::UnqualifiedTokenizer)
    ));
}

struct FixtureRenderer;
impl ExactPromptRenderer for FixtureRenderer {
    fn render(
        &self,
        snapshot: &ContextSnapshot,
    ) -> cowork_model_accounting::Result<RenderedPrompt> {
        Ok(RenderedPrompt {
            model: snapshot.model.clone(),
            logical_request_sha256: digest(snapshot)?,
            text: snapshot
                .groups
                .iter()
                .flat_map(|g| &g.messages)
                .map(|m| m["content"].as_str().unwrap())
                .collect::<Vec<_>>()
                .join(" "),
        })
    }
}

fn context_fixture(tokenizer: &QualifiedTokenizer) -> (ContextSnapshot, CompactionAuthority) {
    let snapshot = ContextSnapshot {
        run_id: "run".into(),
        run_revision: 7,
        scope_sha256: sha256(b"frozen rights and pending accounting"),
        model: tokenizer.model().clone(),
        groups: vec![
            ContextGroup {
                id: "system-and-pending-effect".into(),
                retention: ContextRetention::Required,
                messages: vec![json!({"role":"system","content":"system keep"})],
            },
            ContextGroup {
                id: "complete-old-history".into(),
                retention: ContextRetention::ConfirmedHistory,
                messages: vec![
                    json!({"role":"user","content":"history history history"}),
                    json!({"role":"assistant","content":"hello"}),
                ],
            },
            ContextGroup {
                id: "current-user".into(),
                retention: ContextRetention::Required,
                messages: vec![json!({"role":"user","content":"world"})],
            },
        ],
    };
    let authority = CompactionAuthority {
        operation_id: "explicit-compaction".into(),
        snapshot_sha256: digest(&snapshot).unwrap(),
        scope_sha256: snapshot.scope_sha256.clone(),
        archive_receipt_sha256: sha256(b"durable exact original archive"),
        removable_groups: BTreeMap::from([(
            snapshot.groups[1].id.clone(),
            digest(&snapshot.groups[1]).unwrap(),
        )]),
    };
    (snapshot, authority)
}

#[test]
fn compaction_removes_only_whole_explicitly_archived_confirmed_history_and_replays() {
    let tokenizer = tokenizer(5);
    let (snapshot, authority) = context_fixture(&tokenizer);
    let receipt = compact(&snapshot, &authority, &tokenizer, &FixtureRenderer, 2).unwrap();
    assert_eq!(
        receipt.retained.groups,
        vec![snapshot.groups[0].clone(), snapshot.groups[2].clone()]
    );
    assert_eq!(receipt.count.input_tokens, 3);
    assert_eq!(receipt.removed_groups, authority.removable_groups);
    assert_eq!(
        compact(&snapshot, &authority, &tokenizer, &FixtureRenderer, 2).unwrap(),
        receipt
    );
    assert_eq!(snapshot.groups.len(), 3); // Caller and original evidence stay untouched.
}

#[test]
fn compaction_rejects_pending_effect_removal_and_changed_revision_or_archive() {
    let tokenizer = tokenizer(5);
    let (mut snapshot, mut authority) = context_fixture(&tokenizer);
    authority.removable_groups.insert(
        snapshot.groups[0].id.clone(),
        digest(&snapshot.groups[0]).unwrap(),
    );
    assert_eq!(
        compact(&snapshot, &authority, &tokenizer, &FixtureRenderer, 2),
        Err(Error::Conflict)
    );
    authority.removable_groups.remove(&snapshot.groups[0].id);
    snapshot.run_revision += 1;
    assert_eq!(
        compact(&snapshot, &authority, &tokenizer, &FixtureRenderer, 2),
        Err(Error::Conflict)
    );
    snapshot.run_revision -= 1;
    authority.archive_receipt_sha256.clear();
    assert_eq!(
        compact(&snapshot, &authority, &tokenizer, &FixtureRenderer, 2),
        Err(Error::Conflict)
    );
}

#[test]
fn required_context_cannot_be_silently_truncated_even_with_valid_archive() {
    let tokenizer = tokenizer(2);
    let (snapshot, authority) = context_fixture(&tokenizer);
    assert_eq!(
        compact(&snapshot, &authority, &tokenizer, &FixtureRenderer, 1),
        Err(Error::ContextLimit)
    );
}

fn price() -> PriceVersion {
    PriceVersion {
        id: "explicit-price".into(),
        version: 1,
        provider_id: "provider".into(),
        model_sha256: sha256(b"model"),
        currency: Currency {
            code: "USD".into(),
            scale: 6,
        },
        input_units_per_million: 1_000_000,
        output_units_per_million: 2_000_000,
        cached_input_units_per_million: Some(500_000),
        source_sha256: sha256(b"approved provider price source version"),
    }
}

fn ledger(limit: u64) -> MonetaryLedger {
    let mut ledger = MonetaryLedger::default();
    ledger.register_price(0, price()).unwrap();
    for scope in [
        PoolScope::Run("run".into()),
        PoolScope::Organization("org".into()),
        PoolScope::Instance("instance".into()),
    ] {
        ledger
            .activate_pool(
                ledger.revision(),
                PoolPolicy {
                    scope,
                    version: 1,
                    currency: price().currency,
                    limit_units: limit,
                },
            )
            .unwrap();
    }
    ledger
}

fn spec(id: &str) -> ReservationSpec {
    let mut pools = vec![
        PoolBinding {
            scope: PoolScope::Run("run".into()),
            version: 1,
        },
        PoolBinding {
            scope: PoolScope::Organization("org".into()),
            version: 1,
        },
        PoolBinding {
            scope: PoolScope::Instance("instance".into()),
            version: 1,
        },
    ];
    pools.sort();
    ReservationSpec {
        attempt_id: id.into(),
        request_sha256: sha256(id.as_bytes()),
        provider_id: "provider".into(),
        model_sha256: sha256(b"model"),
        price_id: "explicit-price".into(),
        price_version: 1,
        input_upper_bound: 10,
        output_upper_bound: 5,
        pools,
    }
}
fn required(spec: &ReservationSpec) -> Vec<PoolScope> {
    spec.pools.iter().map(|p| p.scope.clone()).collect()
}
fn usage(
    input: Option<u64>,
    output: Option<u64>,
    cached: Option<u64>,
    confirmed: bool,
) -> ProviderUsage {
    ProviderUsage {
        input_tokens: input,
        output_tokens: output,
        cached_input_tokens: cached,
        provider_receipt_sha256: confirmed.then(|| sha256(b"exact provider usage receipt")),
    }
}

#[test]
fn exact_price_ceiling_once_cache_subset_and_checked_overflow() {
    let mut p = price();
    p.input_units_per_million = 1;
    p.output_units_per_million = 1;
    p.cached_input_units_per_million = None;
    assert_eq!(p.cost(1, 1, 0).unwrap(), 1);
    assert_eq!(p.cost(1_000_000, 0, 0).unwrap(), 1);
    assert_eq!(p.cost(1_000_001, 0, 0).unwrap(), 2);
    assert_eq!(price().cost(10, 5, 10).unwrap(), 15);
    assert!(price().cost(10, 0, 11).is_err());
    p.input_units_per_million = u64::MAX;
    p.output_units_per_million = u64::MAX;
    assert_eq!(p.cost(u64::MAX, u64::MAX, 0), Err(Error::Overflow));
}

#[test]
fn price_versions_are_immutable_and_old_reservation_keeps_exact_price() {
    let mut l = ledger(100);
    let s = spec("a");
    let scopes = required(&s);
    l.admit(l.revision(), s, &scopes).unwrap();
    let mut p = price();
    p.input_units_per_million = 0;
    assert_eq!(
        l.register_price(l.revision(), p.clone()),
        Err(Error::Conflict)
    );
    p.version = 2;
    l.register_price(l.revision(), p).unwrap();
    assert_eq!(l.reservation("a").unwrap().price, price());
    assert_eq!(
        l.charged_to(&PoolScope::Organization("org".into()))
            .unwrap(),
        20
    );
}

#[test]
fn duplicate_admission_returns_original_receipt_without_dispatch_even_stale() {
    let mut l = ledger(100);
    let s = spec("a");
    let scopes = required(&s);
    let first = l.admit(l.revision(), s.clone(), &scopes).unwrap();
    let revision = l.revision();
    let replay = l.admit(0, s.clone(), &scopes).unwrap();
    assert!(first.newly_reserved);
    assert!(!replay.newly_reserved);
    assert_eq!(replay.receipt, first.receipt);
    assert_eq!(l.revision(), revision);
    let mut changed = s;
    changed.output_upper_bound += 1;
    assert_eq!(
        l.admit(l.revision(), changed, &scopes),
        Err(Error::Conflict)
    );
}

#[test]
fn global_multi_scope_capacity_is_atomic_under_competing_admissions() {
    let ledger = Arc::new(Mutex::new(ledger(20)));
    let gate = Arc::new(Barrier::new(3));
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let ledger = ledger.clone();
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                let mut l = ledger.lock().unwrap();
                let s = spec(&format!("attempt-{i}"));
                let scopes = required(&s);
                let revision = l.revision();
                l.admit(revision, s, &scopes)
            })
        })
        .collect();
    gate.wait();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::BudgetExceeded)))
            .count(),
        1
    );
    let l = ledger.lock().unwrap();
    assert_eq!(l.reservations().len(), 1);
    assert_eq!(
        l.charged_to(&PoolScope::Organization("org".into()))
            .unwrap(),
        20
    );
    l.validate().unwrap();
}

#[test]
fn uncertain_and_partial_usage_never_release_original_money_hold() {
    let mut l = ledger(20);
    let s = spec("a");
    let scopes = required(&s);
    l.admit(l.revision(), s, &scopes).unwrap();
    l.uncertain(l.revision(), "a", "lost-provider-ack").unwrap();
    assert_eq!(l.reservation("a").unwrap().charged_units().unwrap(), 20);
    l.settle(l.revision(), "a", usage(Some(2), None, None, true))
        .unwrap();
    assert!(!l.reservation("a").unwrap().complete_usage());
    assert_eq!(l.reservation("a").unwrap().charged_units().unwrap(), 20);
    let s = spec("b");
    assert_eq!(
        l.admit(l.revision(), s.clone(), &required(&s)),
        Err(Error::BudgetExceeded)
    );
    let snapshot = serde_json::to_vec(&l).unwrap();
    let restored: MonetaryLedger = serde_json::from_slice(&snapshot).unwrap();
    restored.validate().unwrap();
    assert_eq!(restored, l);
}

#[test]
fn complete_receipted_usage_settles_once_and_releases_only_confirmed_difference() {
    let mut l = ledger(20);
    let s = spec("a");
    l.admit(l.revision(), s.clone(), &required(&s)).unwrap();
    let measured = usage(Some(2), Some(1), Some(0), true);
    l.settle(l.revision(), "a", measured.clone()).unwrap();
    assert_eq!(l.reservation("a").unwrap().charged_units().unwrap(), 4);
    let revision = l.revision();
    l.settle(0, "a", measured).unwrap();
    assert_eq!(l.revision(), revision);
    assert_eq!(
        l.settle(l.revision(), "a", usage(Some(1), Some(1), Some(0), true)),
        Err(Error::Conflict)
    );
}

#[test]
fn unreceipted_complete_numbers_and_unknown_cache_are_still_unconfirmed() {
    for measured in [
        usage(Some(1), Some(1), Some(0), false),
        usage(Some(1), Some(1), None, true),
    ] {
        let mut l = ledger(20);
        let s = spec("a");
        l.admit(l.revision(), s.clone(), &required(&s)).unwrap();
        l.settle(l.revision(), "a", measured).unwrap();
        assert!(!l.reservation("a").unwrap().complete_usage());
        assert_eq!(l.reservation("a").unwrap().charged_units().unwrap(), 20);
    }
}

#[test]
fn provider_overrun_is_recorded_without_erasing_reservation_or_allowing_new_admission() {
    let mut l = ledger(100);
    let s = spec("a");
    l.admit(l.revision(), s.clone(), &required(&s)).unwrap();
    l.settle(l.revision(), "a", usage(Some(30), Some(40), Some(0), true))
        .unwrap();
    assert_eq!(l.reservation("a").unwrap().charged_units().unwrap(), 110);
    assert!(l.reservation("a").unwrap().overrun());
    let s = spec("b");
    assert_eq!(
        l.admit(l.revision(), s.clone(), &required(&s)),
        Err(Error::BudgetExceeded)
    );
    l.validate().unwrap();
}

#[test]
fn unpriced_cache_is_unknown_until_explicit_zero_was_received() {
    let mut l = ledger(100);
    let mut p = price();
    p.version = 2;
    p.cached_input_units_per_million = None;
    l.register_price(l.revision(), p).unwrap();
    let mut s = spec("missing-cache");
    s.price_version = 2;
    l.admit(l.revision(), s.clone(), &required(&s)).unwrap();
    l.settle(
        l.revision(),
        &s.attempt_id,
        usage(Some(1), Some(1), None, true),
    )
    .unwrap();
    let reservation = l.reservation(&s.attempt_id).unwrap();
    assert!(!reservation.complete_usage());
    assert_eq!(reservation.charged_units().unwrap(), 20);

    let mut s = spec("explicit-cache-zero");
    s.price_version = 2;
    l.admit(l.revision(), s.clone(), &required(&s)).unwrap();
    l.settle(
        l.revision(),
        &s.attempt_id,
        usage(Some(1), Some(1), Some(0), true),
    )
    .unwrap();
    assert!(l.reservation(&s.attempt_id).unwrap().complete_usage());
    assert_eq!(
        l.reservation(&s.attempt_id)
            .unwrap()
            .charged_units()
            .unwrap(),
        3
    );
    l.validate().unwrap();
}

#[test]
fn uncertain_received_overrun_retains_partial_cost_and_freezes_shared_pools() {
    let mut l = ledger(1000);
    let s = spec("a");
    l.admit(l.revision(), s.clone(), &required(&s)).unwrap();
    let evidence = usage(None, Some(40), None, true);
    l.uncertain_with_usage(l.revision(), "a", "incomplete", Some(evidence.clone()))
        .unwrap();
    let reservation = l.reservation("a").unwrap();
    assert!(!reservation.complete_usage());
    assert!(reservation.overrun());
    assert_eq!(reservation.charged_units().unwrap(), 80);
    let revision = l.revision();
    l.uncertain_with_usage(0, "a", "incomplete", Some(evidence))
        .unwrap();
    assert_eq!(l.revision(), revision);
    let original = l.clone();
    assert_eq!(
        l.uncertain_with_usage(
            l.revision(),
            "a",
            "incomplete",
            Some(usage(None, Some(1), None, true))
        ),
        Err(Error::Conflict)
    );
    assert_eq!(l, original);
    let b = spec("b");
    assert_eq!(
        l.admit(l.revision(), b.clone(), &required(&b)),
        Err(Error::BudgetExceeded)
    );
    let bytes = serde_json::to_vec(&l).unwrap();
    let reopened: MonetaryLedger = serde_json::from_slice(&bytes).unwrap();
    reopened.validate().unwrap();
    assert_eq!(reopened, l);
    assert_eq!(
        reopened.reservation("a").unwrap().charged_units().unwrap(),
        80
    );
}

#[test]
fn legacy_uncertainty_bytes_and_invalid_partial_receipt_are_preserved() {
    let bytes = br#"{"state":"uncertain","reason":"lost-provider-ack"}"#;
    let state: ReservationState = serde_json::from_slice(bytes).unwrap();
    assert_eq!(serde_json::to_vec(&state).unwrap(), bytes);
    let mut l = ledger(100);
    let s = spec("a");
    l.admit(l.revision(), s.clone(), &required(&s)).unwrap();
    let original = l.clone();
    let mut bad = usage(None, Some(40), None, true);
    bad.provider_receipt_sha256 = Some("forged".into());
    assert!(
        l.uncertain_with_usage(l.revision(), "a", "incomplete", Some(bad))
            .is_err()
    );
    assert_eq!(l, original);
    assert!(
        l.uncertain_with_usage(
            l.revision(),
            "a",
            "incomplete",
            Some(usage(Some(1), None, Some(2), true))
        )
        .is_err()
    );
    assert_eq!(l, original);
}

#[test]
fn required_org_scope_cannot_be_omitted_and_stale_new_admission_changes_nothing() {
    let mut l = ledger(100);
    let before = l.clone();
    let mut s = spec("a");
    let scopes = required(&s);
    s.pools
        .retain(|p| !matches!(p.scope, PoolScope::Organization(_)));
    assert_eq!(l.admit(l.revision(), s, &scopes), Err(Error::Conflict));
    assert_eq!(l, before);
    let s = spec("b");
    assert_eq!(
        l.admit(0, s.clone(), &required(&s)),
        Err(Error::StaleRevision)
    );
    assert_eq!(l, before);
}

#[test]
fn lower_new_policy_keeps_charges_from_all_historical_policy_versions() {
    let mut l = ledger(100);
    let s = spec("a");
    l.admit(l.revision(), s.clone(), &required(&s)).unwrap();
    l.activate_pool(
        l.revision(),
        PoolPolicy {
            scope: PoolScope::Organization("org".into()),
            version: 2,
            currency: price().currency,
            limit_units: 19,
        },
    )
    .unwrap();
    assert_eq!(
        l.charged_to(&PoolScope::Organization("org".into()))
            .unwrap(),
        20
    );
    let mut s = spec("b");
    s.pools
        .iter_mut()
        .find(|p| matches!(p.scope, PoolScope::Organization(_)))
        .unwrap()
        .version = 2;
    assert_eq!(
        l.admit(l.revision(), s.clone(), &required(&s)),
        Err(Error::BudgetExceeded)
    );
}

#[test]
fn restored_ledger_rejects_forged_reservation_or_price_receipt() {
    let mut l = ledger(100);
    let s = spec("a");
    l.admit(l.revision(), s.clone(), &required(&s)).unwrap();
    let mut value = serde_json::to_value(&l).unwrap();
    value["reservations"]["a"]["receipt"]["reserved_units"] = json!(0);
    let forged: MonetaryLedger = serde_json::from_value(value).unwrap();
    assert_eq!(forged.validate(), Err(Error::Conflict));
}

#[test]
fn separate_run_pools_still_compete_for_one_shared_organization_capacity() {
    let mut l = ledger(100);
    l.activate_pool(
        l.revision(),
        PoolPolicy {
            scope: PoolScope::Organization("org".into()),
            version: 2,
            currency: price().currency,
            limit_units: 20,
        },
    )
    .unwrap();
    for run in ["run-a", "run-b"] {
        l.activate_pool(
            l.revision(),
            PoolPolicy {
                scope: PoolScope::Run(run.into()),
                version: 1,
                currency: price().currency,
                limit_units: 100,
            },
        )
        .unwrap();
    }
    let shared = Arc::new(Mutex::new(l));
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = ["run-a", "run-b"]
        .into_iter()
        .map(|run| {
            let shared = shared.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut s = spec(run);
                for pool in &mut s.pools {
                    match &mut pool.scope {
                        PoolScope::Run(id) => *id = run.into(),
                        PoolScope::Organization(_) => pool.version = 2,
                        _ => {}
                    }
                }
                s.pools.sort();
                barrier.wait();
                let mut l = shared.lock().unwrap();
                let revision = l.revision();
                l.admit(revision, s.clone(), &required(&s))
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::BudgetExceeded)))
            .count(),
        1
    );
    let l = shared.lock().unwrap();
    assert_eq!(
        l.charged_to(&PoolScope::Organization("org".into()))
            .unwrap(),
        20
    );
    assert_eq!(
        l.charged_to(&PoolScope::Run("run-a".into())).unwrap()
            + l.charged_to(&PoolScope::Run("run-b".into())).unwrap(),
        20
    );
}

#[test]
fn failed_settlement_overflow_keeps_original_hold_and_revision() {
    let mut l = ledger(100);
    let s = spec("a");
    l.admit(l.revision(), s.clone(), &required(&s)).unwrap();
    let original = l.clone();
    assert_eq!(
        l.settle(
            l.revision(),
            "a",
            usage(Some(u64::MAX), Some(u64::MAX), Some(0), true)
        ),
        Err(Error::Overflow)
    );
    assert_eq!(l, original);
    assert_eq!(l.reservation("a").unwrap().charged_units().unwrap(), 20);
}

#[test]
fn currencies_unknown_price_and_policy_downgrades_never_change_the_ledger() {
    let mut l = ledger(100);
    let original = l.clone();
    let mut s = spec("a");
    s.price_version = 999;
    assert_eq!(
        l.admit(l.revision(), s.clone(), &required(&s)),
        Err(Error::PriceUnknown)
    );
    assert_eq!(l, original);
    let mut changed = price().currency;
    changed.code = "EUR".into();
    assert_eq!(
        l.activate_pool(
            l.revision(),
            PoolPolicy {
                scope: PoolScope::Organization("org".into()),
                version: 2,
                currency: changed,
                limit_units: 100
            }
        ),
        Err(Error::Conflict)
    );
    l.activate_pool(
        l.revision(),
        PoolPolicy {
            scope: PoolScope::Organization("org".into()),
            version: 2,
            currency: price().currency,
            limit_units: 10,
        },
    )
    .unwrap();
    let current = l.clone();
    assert_eq!(
        l.activate_pool(
            l.revision(),
            PoolPolicy {
                scope: PoolScope::Organization("org".into()),
                version: 1,
                currency: price().currency,
                limit_units: 100
            }
        ),
        Err(Error::Conflict)
    );
    assert_eq!(l, current);
}
