import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'

export type LegacyMemoryProviderConfig = {
  id: string
  name: string
  provider_type: string
  config_json: string
  enabled: boolean
  last_sync_at: string | null
  created_at: string
  updated_at: string
}

export type LegacyToolGatewayConfig = {
  id: string
  tool_type: string
  name: string
  config_json: string
  enabled: boolean
  created_at: string
  updated_at: string
}

const LOCAL_PROVIDER_KEY = 'open-cowork-providers-local'
const LOCAL_GATEWAY_KEY = 'open-cowork-gateway'

function parseArray<T>(storageKey: string): T[] {
  try {
    const parsed = JSON.parse(localStorage.getItem(storageKey) ?? '[]')
    return Array.isArray(parsed) ? parsed as T[] : []
  } catch {
    return []
  }
}

function clearStorageKey(storageKey: string): void {
  try { localStorage.removeItem(storageKey) } catch { /* noop */ }
}

let volatileMemoryProviders = parseArray<LegacyMemoryProviderConfig>(LOCAL_PROVIDER_KEY)
let volatileToolGateways = parseArray<LegacyToolGatewayConfig>(LOCAL_GATEWAY_KEY)
let providerMigration: Promise<void> | null = null
let gatewayMigration: Promise<void> | null = null

export function getVolatileMemoryProviders(): LegacyMemoryProviderConfig[] {
  return volatileMemoryProviders
}

export function setVolatileMemoryProviders(providers: LegacyMemoryProviderConfig[]): void {
  volatileMemoryProviders = providers
}

export function getVolatileToolGateways(): LegacyToolGatewayConfig[] {
  return volatileToolGateways
}

export function setVolatileToolGateways(gateways: LegacyToolGatewayConfig[]): void {
  volatileToolGateways = gateways
}

export async function migrateLegacyMemoryProviderConfigs(): Promise<void> {
  const legacyProviders = parseArray<LegacyMemoryProviderConfig>(LOCAL_PROVIDER_KEY)
  if (legacyProviders.length > 0) {
    volatileMemoryProviders = Array.from(new Map(
      [...volatileMemoryProviders, ...legacyProviders].map((provider) => [provider.id, provider]),
    ).values())
  }
  if (!hasTauriRuntime() || volatileMemoryProviders.length === 0) {
    clearStorageKey(LOCAL_PROVIDER_KEY)
    return
  }
  if (!providerMigration) {
    providerMigration = Promise.all(volatileMemoryProviders.map((provider) => safeInvoke(
      'memory_provider_upsert',
      {
        id: provider.id,
        name: provider.name,
        providerType: provider.provider_type,
        configJson: provider.config_json,
        enabled: provider.enabled,
      },
    ))).then(() => {
      volatileMemoryProviders = []
      clearStorageKey(LOCAL_PROVIDER_KEY)
      providerMigration = null
    }).catch((error) => {
      providerMigration = null
      throw error
    })
  }
  await providerMigration
}

export async function migrateLegacyToolGatewayConfigs(): Promise<void> {
  const legacyGateways = parseArray<LegacyToolGatewayConfig>(LOCAL_GATEWAY_KEY)
  if (legacyGateways.length > 0) {
    volatileToolGateways = Array.from(new Map(
      [...volatileToolGateways, ...legacyGateways].map((gateway) => [gateway.id, gateway]),
    ).values())
  }
  if (!hasTauriRuntime() || volatileToolGateways.length === 0) {
    clearStorageKey(LOCAL_GATEWAY_KEY)
    return
  }
  if (!gatewayMigration) {
    gatewayMigration = Promise.all(volatileToolGateways.map((gateway) => safeInvoke(
      'tool_gateway_upsert',
      {
        id: gateway.id,
        toolType: gateway.tool_type,
        name: gateway.name,
        configJson: gateway.config_json,
        enabled: gateway.enabled,
      },
    ))).then(() => {
      volatileToolGateways = []
      clearStorageKey(LOCAL_GATEWAY_KEY)
      gatewayMigration = null
    }).catch((error) => {
      gatewayMigration = null
      throw error
    })
  }
  await gatewayMigration
}
