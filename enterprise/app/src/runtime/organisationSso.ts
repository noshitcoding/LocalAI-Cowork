import { z } from 'zod'

export function organisationId(value:string|null|undefined):string|undefined {
  const parsed=z.uuid().safeParse(value?.trim())
  return parsed.success?parsed.data:undefined
}
export function ssoPath(org?:string):string {
  return org===undefined?'/api/v1/auth/oidc':`/api/v1/auth/organisations/${z.uuid().parse(org)}/oidc`
}
