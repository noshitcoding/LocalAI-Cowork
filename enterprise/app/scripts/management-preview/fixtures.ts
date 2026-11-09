export const uid = (value: number) => `10000000-0000-4000-8000-${String(value).padStart(12, '0')}`
export const now = '2026-10-17T06:00:00Z'
export const userId = uid(90)
export const policy = { revision: 3, extensions: 'catalog', content_access: 'support_grant', minimum_isolation: 'container', allow_laptop_full_access: false, allow_offline_continuation: true, allowed_network_origins: ['https://docs.example.invalid'], limits: { cpu_millis: 8000, memory_mib: 16384, storage_gib: 100, instances: 10, concurrent_runs: 4 }, audit_retention_days: 365, content_logging: false }
export const organisations = [
  { id: uid(1), name: 'Nordlicht Produktteam', role: 'admin', policy, created_at: now },
  { id: uid(2), name: 'Forschung & Entwicklung', role: 'admin', policy, created_at: now },
]
export const instances = [
  { id: uid(11), organisation_id: uid(1), owner_user_id: userId, name: 'Produktteam · Arbeitsserver', backend: 'compose', state: 'ready', policy: { ...policy, minimum_isolation: 'virtual_machine' }, revision: 7, current_release: 'a'.repeat(64), provisioning_release: 'a'.repeat(64), rollout_id: null, execution_required: true, can_manage: true, can_open: true, can_open_support: false, available_actions: ['stop', 'backup', 'upgrade'], created_at: now },
  { id: uid(12), organisation_id: uid(1), owner_user_id: userId, name: 'Analyse · Staging', backend: 'compose', state: 'stopped', policy, revision: 4, current_release: 'a'.repeat(64), provisioning_release: 'a'.repeat(64), rollout_id: null, execution_required: true, can_manage: true, can_open: false, can_open_support: false, available_actions: ['start', 'backup', 'restore', 'upgrade', 'delete'], created_at: now },
]
export const members = [
  { user_id: userId, display_name: 'Alex Weber', email: 'alex@example.invalid', role: 'admin', content_admin: false, sso_provider_id: null },
  { user_id: uid(91), display_name: 'Samira Klein', email: 'samira@example.invalid', role: 'member', content_admin: false, sso_provider_id: null },
  { user_id: uid(92), display_name: 'Robin Fischer', email: 'robin@example.invalid', role: 'auditor', content_admin: false, sso_provider_id: null },
]
export const templates = [
  { id: uid(31), revision: 2, name: 'Python & Dokumente', backend: 'open_shell', isolation: 'virtual_machine', image_reference: `registry.example.invalid/research@sha256:${'c'.repeat(64)}`, enabled: true },
  { id: uid(32), revision: 1, name: 'Linux-Werkzeuge', backend: 'docker', isolation: 'virtual_machine', image_reference: `registry.example.invalid/tools@sha256:${'d'.repeat(64)}`, enabled: false },
]
export const environments = [{ id: uid(21), revision: 4, state: 'running', spec: { name: 'Recherche · Python', backend: 'open_shell', isolation: 'virtual_machine', persistent: true, shared_workspace: false, limits: { cpu_millis: 2000, memory_mib: 4096, storage_gib: 10 } }, runtime: { sandbox_id: 'preview-openshell-runtime', container_id: 'e'.repeat(64) } }]
export const controllers = [{ id: uid(41), backend: 'open_shell', revision: 2, enabled: true, specification: { vm_id: uid(42), limits: { cpu_millis: 8000, memory_mib: 16384, storage_gib: 100 } } }]
export const rollouts = [{ id: uid(51), organisation_id: uid(1), name: 'Oktober · Werkzeugupdate', revision: 2, release: 'b'.repeat(64), batch_size: 1, state: 'succeeded', error_code: null, created_at: now, updated_at: now, targets: [{ instance_id: uid(12), instance_name: 'Analyse · Staging', ordinal: 0, state: 'succeeded', next_action: null, error_code: null }] }]
export const delivery = { enforced: false as const, state: 'awaiting_consumer' as const, message: 'Zuweisungen sind gespeichert. Die Laufzeiten müssen den Ressourcenkatalog noch übernehmen.', supported_model_subset: { consumer_kind: 'model_provider' as const, version: 1 as const, protocols: ['openai_compatible' as const], scopes: ['organisation' as const, 'instance' as const], enforcement: 'fresh_resolution_only' as const, requires_instance_upgrade: true as const, immediate_server_runs_only: true as const } }
const resource = (n: number, kind: string, name: string, description: string, configuration: Record<string, unknown>) => ({ id: uid(n), kind, name, description, configuration, secret_reference: null, enabled: true, revision: 1, created_by: userId, updated_by: userId, created_at: now, updated_at: now, deleted_at: null, available: true })
export const resources = [
  resource(61, 'endpoint', 'Team-Modellserver', 'Interner OpenAI-kompatibler Endpunkt', { base_url: 'https://models.example.invalid/v1', protocol: 'openai_compatible' }),
  resource(62, 'model', 'Team-Assistent', 'Modellprofil für alltägliche Aufgaben', { model_id: 'preview-assistant', endpoint_resource_id: uid(61), context_window: 32768, max_output_tokens: 4096 }),
  resource(63, 'mcp', 'Dokumentenwissen', 'Recherche in freigegebenen Teamdokumenten', { transport: 'streamable_http', url: 'https://knowledge.example.invalid/mcp', args: [] }),
  resource(64, 'skill', 'Rechercheleitfaden', 'Quellen prüfen und Ergebnisse knapp zusammenfassen', { instructions: 'Prüfe Quellen und nenne Unsicherheiten.' }),
  resource(65, 'agent_role', 'Recherche-Assistent', 'Arbeitsrolle für quellenbasierte Recherche', { system_prompt: 'Arbeite sorgfältig und nenne deine Quellen.', resource_ids: [uid(62), uid(63)] }),
]

export const walletList = (organisationId: string) => ({ organisation_id: organisationId, currency: 'EUR', settings_revision: 1, currency_locked: true, coverage: 'managed_model_proxy', wallets: members.map((member, index) => ({ organisation_id: organisationId, user_id: member.user_id, email: member.email, display_name: member.display_name, currency: 'EUR', revision: 3, enabled: index !== 2, balance_micros: index === 0 ? 35000000 : index === 1 ? 12500000 : 0, reserved_micros: index === 0 ? 2500000 : 0, available_micros: index === 0 ? 32500000 : index === 1 ? 12500000 : 0, monthly_limit_micros: index === 0 ? 50000000 : 20000000, monthly_spent_micros: index === 0 ? 15000000 : 7500000, monthly_reserved_micros: index === 0 ? 2500000 : 0, period_start: '2026-10-01T00:00:00Z', enforcement: { state: index !== 2 ? 'enforced' : 'disabled', coverage: 'server_managed_models', automatic_charging: true } })) })
export const walletDetail = (organisationId: string, selectedUserId: string) => {
  const wallet = walletList(organisationId).wallets.find(item => item.user_id === selectedUserId)!
  const allocation = wallet.balance_micros + wallet.monthly_spent_micros
  return { ...wallet, entries: wallet.balance_micros ? [
    { id: uid(101), request_id: uid(111), kind: 'usage_charge', amount_micros: wallet.monthly_spent_micros, balance_after_micros: wallet.balance_micros, reason: 'TESTDATEN · Verbuchte Modellnutzung', created_at: now, actor_user_id: null },
    { id: uid(102), request_id: uid(112), kind: 'credit', amount_micros: allocation, balance_after_micros: allocation, reason: 'TESTDATEN · Interne Zuteilung Oktober', created_at: '2026-10-01T08:00:00Z', actor_user_id: userId },
  ] : [], reservations: selectedUserId === userId ? [{ id: uid(121), state: 'held', maximum_micros: 2500000, charged_micros: null, created_at: now }] : [], has_more: false }
}
