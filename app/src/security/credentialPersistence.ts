type SecretProfile = { apiKey: string }
type SecretMcpServer = { env: Record<string, string> }
type SecretConnector = { apiKey?: string; webhookUrl?: string }
type SecretCrew = {
  providerProfiles: {
    openAICompatible: SecretProfile
    openRouter: SecretProfile
  }
}

export function sanitizeEnvironmentForPersistence(
  environment: Record<string, string>,
): Record<string, string> {
  return Object.fromEntries(Object.keys(environment).map((key) => [key, '']))
}

export function sanitizeProfilesForPersistence<T extends SecretProfile>(profiles: T[]): T[] {
  return profiles.map((profile) => ({ ...profile, apiKey: '' }))
}

export function sanitizeMcpServerForPersistence<T extends SecretMcpServer>(server: T): T {
  return {
    ...server,
    env: sanitizeEnvironmentForPersistence(server.env ?? {}),
  }
}

export function sanitizeConnectorsForPersistence<T extends SecretConnector>(connectors: T[]): T[] {
  return connectors.map((connector) => ({
    ...connector,
    apiKey: connector.apiKey === undefined ? undefined : '',
    webhookUrl: connector.webhookUrl === undefined ? undefined : '',
  }))
}

export function sanitizeCrewsForPersistence<T extends SecretCrew>(crews: T[]): T[] {
  return crews.map((crew) => ({
    ...crew,
    providerProfiles: {
      ...crew.providerProfiles,
      openAICompatible: {
        ...crew.providerProfiles.openAICompatible,
        apiKey: '',
      },
      openRouter: {
        ...crew.providerProfiles.openRouter,
        apiKey: '',
      },
    },
  }))
}

export function sanitizeEngineConfigForPersistence<T extends SecretProfile>(config: T): T {
  return { ...config, apiKey: '' }
}
