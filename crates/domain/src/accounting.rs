use super::*;

pub fn validate_budgets(budgets: &Budgets) -> Result<()> {
    if !(1..=86_400).contains(&budgets.max_seconds)
        || !(1..=4096).contains(&budgets.max_steps)
        || !(1..=1_000_000).contains(&budgets.max_output_tokens)
        || !(1024..=1_000_000_000).contains(&budgets.max_total_tokens)
    {
        return Err(Error::Validation(
            "Execution budgets are outside the supported range".into(),
        ));
    }
    Ok(())
}

pub fn measured_tokens(usage: &Usage) -> u64 {
    usage
        .input_tokens
        .unwrap_or(0)
        .saturating_add(usage.output_tokens.unwrap_or(0))
}

pub fn token_budget(run: &RunRecord) -> TokenBudgetStatus {
    let mut result = TokenBudgetStatus {
        limit: run.spec.budgets.max_total_tokens,
        ..Default::default()
    };
    for attempt in &run.model_attempts {
        let measured = measured_tokens(&attempt.usage);
        let charged = if attempt.state == ModelAttemptState::Measured {
            measured
        } else {
            let held = attempt.reserved_tokens.max(measured);
            result.unconfirmed_tokens = result.unconfirmed_tokens.saturating_add(held);
            held
        };
        result.charged_tokens = result.charged_tokens.saturating_add(charged);
        result.limit_exceeded |= attempt.limit_exceeded;
    }
    result.limit_exceeded |= result.charged_tokens > result.limit;
    result.remaining_tokens = result.limit.saturating_sub(result.charged_tokens);
    result
}

/// Admission reserves the complete configured context window, not a guessed tokenizer count.
/// Remote providers must enforce their own limits; their reported overruns block further actions.
pub fn reserve_model(
    run: &mut RunRecord,
    request_sha256: &str,
    id: &str,
    timestamp: i64,
) -> Result<ModelAttempt> {
    reserve_model_tokens(run, request_sha256, id, timestamp, None)
}
/// Called only by the trusted finalized-request adapter after artifact parity
/// and complete prompt counting. Unqualified providers retain the full window.
pub fn reserve_model_tokens(
    run: &mut RunRecord,
    request_sha256: &str,
    id: &str,
    timestamp: i64,
    exact_total: Option<u64>,
) -> Result<ModelAttempt> {
    validate_budgets(&run.spec.budgets)?;
    if run.state != RunState::Running
        || run.spec.file_action.is_some()
        || run
            .model_attempts
            .iter()
            .any(|a| a.state == ModelAttemptState::InFlight)
    {
        return Err(Error::Conflict);
    }
    if request_sha256.len() != 64 || !request_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Validation("Invalid request digest".into()));
    }
    let window = run
        .spec
        .provider
        .as_ref()
        .ok_or(Error::Forbidden)?
        .context_tokens as u64;
    let reserved_tokens = exact_total.unwrap_or(window);
    if reserved_tokens == 0
        || reserved_tokens > window
        || exact_total.is_some() != run.spec.model_binding.is_some()
    {
        return Err(Error::Forbidden);
    }
    let balance = token_budget(run);
    if balance.limit_exceeded
        || reserved_tokens > balance.remaining_tokens
        || run.steps >= run.spec.budgets.max_steps
    {
        return Err(Error::BudgetExceeded);
    }
    run.steps += 1;
    let attempt = ModelAttempt {
        id: id.into(),
        step: run.steps,
        request_sha256: Some(request_sha256.into()),
        reserved_tokens,
        state: ModelAttemptState::InFlight,
        usage: Usage::default(),
        started_at: timestamp,
        finished_at: None,
        limit_exceeded: false,
    };
    run.model_attempts.push(attempt.clone());
    Ok(attempt)
}

pub fn settle_model(
    run: &mut RunRecord,
    attempt_id: &str,
    usage: &Usage,
    timestamp: i64,
) -> Result<()> {
    let attempt = run
        .model_attempts
        .iter_mut()
        .find(|a| a.id == attempt_id)
        .ok_or(Error::Conflict)?;
    if attempt.state != ModelAttemptState::InFlight {
        return Err(Error::Conflict);
    }
    attempt.usage = usage.clone();
    attempt.state =
        if usage.input_tokens.is_some() && usage.output_tokens.is_some() && !usage.incomplete {
            ModelAttemptState::Measured
        } else {
            ModelAttemptState::Unmeasured
        };
    attempt.finished_at = Some(timestamp);
    attempt.limit_exceeded = measured_tokens(usage) > attempt.reserved_tokens
        || usage
            .output_tokens
            .is_some_and(|n| n > run.spec.budgets.max_output_tokens as u64);
    add_reported_usage(&mut run.usage, usage);
    Ok(())
}

pub fn interrupt_models(run: &mut RunRecord, timestamp: i64) -> Vec<String> {
    let mut interrupted = vec![];
    for attempt in &mut run.model_attempts {
        if attempt.state == ModelAttemptState::InFlight {
            attempt.state = ModelAttemptState::Interrupted;
            attempt.finished_at = Some(timestamp);
            interrupted.push(attempt.id.clone());
        }
    }
    if !interrupted.is_empty() {
        run.usage.incomplete = true;
    }
    interrupted
}

pub fn add_reported_usage(total: &mut Usage, usage: &Usage) {
    if let Some(n) = usage.input_tokens {
        total.input_tokens = Some(total.input_tokens.unwrap_or(0).saturating_add(n));
    }
    if let Some(n) = usage.output_tokens {
        total.output_tokens = Some(total.output_tokens.unwrap_or(0).saturating_add(n));
    }
    total.incomplete |=
        usage.incomplete || usage.input_tokens.is_none() || usage.output_tokens.is_none();
    // Costs remain unknown until a versioned price source is available.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_budget_extension_preserves_old_request_identity_and_partial_totals_are_labeled() {
        let old = json!({"order_id":"o","base_revision":1,"idempotency_key":"k","prompt":"p","provider_id":"p","provider_revision":1,"access_mode":"confirm_mutations"});
        let input: CreateRun = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(canonical_hash(&input), canonical_hash(&old));
        let mut changed = input;
        changed.budgets.max_total_tokens = 32768;
        assert_ne!(canonical_hash(&changed), canonical_hash(&old));
        let mut total = Usage::default();
        add_reported_usage(
            &mut total,
            &Usage {
                input_tokens: Some(u64::MAX),
                output_tokens: Some(1),
                ..Default::default()
            },
        );
        add_reported_usage(
            &mut total,
            &Usage {
                input_tokens: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(total.input_tokens, Some(u64::MAX));
        assert!(total.incomplete);
        add_reported_usage(
            &mut total,
            &Usage {
                input_tokens: Some(0),
                output_tokens: Some(0),
                ..Default::default()
            },
        );
        assert!(total.incomplete);
    }
}
