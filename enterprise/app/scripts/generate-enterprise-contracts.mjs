import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { resolve, dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'
import { z } from 'zod'

const app = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const root = resolve(app, '..')
const output = join(root, 'contracts/generated/enterprise-v3')
const check = process.argv.includes('--check')
const vite = await createServer({ root: app, logLevel: 'silent', appType: 'custom', server: { middlewareMode: true } })
let registry
try {
  ({ enterpriseContractRegistry: registry } = await vite.ssrLoadModule('/src/runtime/enterpriseContracts.ts'))
  const { authTokensSchema } = await vite.ssrLoadModule('/src/runtime/authContracts.ts')
  registry = { ...registry, AuthTokens: authTokensSchema }
}
finally { await vite.close() }
const schemas = Object.fromEntries(Object.entries(registry).map(([name, schema]) => {
  const value = z.toJSONSchema(schema, { target: 'draft-2020-12' })
  delete value.$schema
  return [name, value]
}))
schemas.Named = z.toJSONSchema(z.object({ name: z.string().min(1).max(120) }).strict())
schemas.MemberRequest = z.toJSONSchema(z.object({ email: z.string(), role: z.enum(['admin', 'member', 'auditor']), content_admin: z.boolean().optional() }).strict())
schemas.TemplateRequest = z.toJSONSchema(z.object({ name: z.string(), backend: registry.Environment.shape.backend, isolation: registry.Environment.shape.isolation, image_reference: z.string() }).strict())
const source = (await readFile(join(root, 'server/cowork-server/src/management.rs'), 'utf8')).split('pub fn provisioner_router')[0]
const paths = {}
for (const match of source.matchAll(/\.route\(\s*"([^"]+)"\s*,([\s\S]*?)(?=\n\s*\.(?:route|route_layer)|\n\s*})/g)) {
  const path = '/management/v1' + match[1]
  paths[path] = {}
  for (const method of ['get', 'post', 'put', 'delete']) {
    if (!new RegExp(`\\b${method}\\s*\\(`).test(match[2])) continue
    const handler = match[2].match(new RegExp(`\\b${method}\\s*\\(\\s*(\\w+)`))?.[1]
    const request = { update_mfa_policy:'UpdateOrganisationMfaPolicy',set_execution_budget:'SetExecutionBudget', update_sso:'UpdateOrganisationSso', create_invitation: 'CreateOrganisationInvitation', accept_invitation: 'OrganisationInvitationToken', create_organisation: 'Named', create_rollout: 'CreateRollout', control_rollout: 'ControlRollout', update_policy: 'PolicyRevision', set_member: 'MemberRequest', create_instance: 'CreateInstance', request_recovery: 'OperationRecoveryRequest', request_operation: 'ActionRequest', set_instance_member: 'InstanceMember', create_template: 'TemplateRequest', create_environment: 'Environment' }[handler]
    const response = { mfa_policy:'OrganisationMfaPolicy',update_mfa_policy:'OrganisationMfaReceipt',execution_capacity:'ExecutionCapacity', set_execution_budget:'ExecutionBudget', sso_settings:'OrganisationSsoSettings', update_sso:'OrganisationSsoSettings', create_invitation: 'OrganisationInvitationSecret', list_invitations: 'OrganisationInvitation', accept_invitation: 'OrganisationInvitationAccepted', organisations: 'OrganisationList', list_rollouts: 'Rollout', instances: 'Instance', backups: 'InstanceBackup', instance_releases: 'InstanceRelease', provisioning_releases: 'ProvisioningRelease', instance_rollbacks: 'InstanceRollback', instance_members: 'InstanceMemberRecord', instance_member_candidates: 'InstanceMemberCandidate', set_instance_member: 'InstanceMember', update_policy: 'PolicyRevision', request_launch: 'InstanceLaunch' }[handler]
    paths[path][method] = {
      operationId: handler,
      security: [{ session: [] }],
      parameters: [...path.matchAll(/\{([^}]+)}/g)].map(m => ({ name: m[1], in: 'path', required: true, schema: { type: 'string', format: 'uuid' } })),
      ...(request ? { requestBody: { required: true, content: { 'application/json': { schema: { $ref: `#/components/schemas/${request}` } } } } } : {}),
      responses: {
        [handler === 'create_invitation' ? 201 : 200]: { description: 'Successful response', ...(response ? { content: { 'application/json': { schema: ['list_invitations', 'list_rollouts', 'instances', 'backups', 'instance_releases', 'provisioning_releases', 'instance_rollbacks', 'instance_members', 'instance_member_candidates'].includes(handler) ? { type: 'array', items: { $ref: `#/components/schemas/${response}` } } : { $ref: `#/components/schemas/${response}` } } } } : {}) },
        401: { description: 'Missing or expired session' }, 404: { description: 'Resource absent or not accessible in this organisation' },
        409: { description: 'Revision, state or idempotency conflict' }, 422: { description: 'Invalid request or policy exceeds the parent scope' }, 429: { description: 'Quota exceeded' },
      },
    }
  }
}
if (Object.keys(paths).length !== (source.match(/\.route\(/g) ?? []).length) throw new Error('Enterprise route extraction failed')
const publicSource = (await readFile(join(root, 'server/cowork-server/src/management/invitations.rs'), 'utf8')).split('fn digest')[0]
const publicRoutes = [...publicSource.matchAll(/\.route\(\s*"([^"]+)"\s*,\s*post\((\w+)\)/g)]
if (publicRoutes.length !== 2) throw new Error('Invitation route extraction failed')
for (const [, path, handler] of publicRoutes) {
  const register = handler === 'register_invitation'
  if (!register && handler !== 'inspect_invitation') throw new Error(`Unknown invitation handler: ${handler}`)
  paths['/api/v1' + path] = { post: {
    operationId: handler, security: [],
    requestBody: { required: true, content: { 'application/json': { schema: { $ref: `#/components/schemas/${register ? 'RegisterOrganisationInvitation' : 'OrganisationInvitationToken'}` } } } },
    responses: {
      [register ? 201 : 200]: { description: 'Invitation-bound response; Cache-Control: no-store. Browser registration returns its refresh token in an HttpOnly cookie.', content: { 'application/json': { schema: { $ref: `#/components/schemas/${register ? 'AuthTokens' : 'OrganisationInvitationPreview'}` } } } },
      401: { description: 'Invitation expired, revoked, consumed or unavailable' },
      409: { description: 'Existing identity must authenticate before accepting' },
      422: { description: 'Invalid request' },
    },
  } }
}
paths['/api/v1/content-access'] = { get: { operationId: 'instance_content_access', security: [{ session: [] }], responses: { 200: { description: 'Effective administrative content rule in this instance', content: { 'application/json': { schema: { $ref: '#/components/schemas/InstanceContentAccess' } } } }, 401: { description: 'No active authorised instance session' } } } }
paths['/api/v1/auth/assurance'] = { get: { operationId:'authentication_assurance',security:[{session:[]}],responses:{200:{description:'Session-bound second-factor requirements; Cache-Control: no-store',content:{'application/json':{schema:{$ref:'#/components/schemas/AuthenticationAssurance'}}}},401:{description:'No active user session'}} } }
paths['/api/v1/auth/assurance/verify'] = { post: { operationId:'verify_second_factor',security:[{session:[]}],requestBody:{required:true,content:{'application/json':{schema:{$ref:'#/components/schemas/VerifySecondFactor'}}}},responses:{200:{description:'Only the current session is verified for twelve hours',content:{'application/json':{schema:{$ref:'#/components/schemas/AuthenticationAssurance'}}}},401:{description:'Invalid or consumed code'},409:{description:'No authenticator enrolled'},422:{description:'Invalid code format'},429:{description:'Five invalid attempts within five minutes; temporarily locked'}} } }
for(const [path,method] of [['/api/v1/auth/assurance','get'],['/api/v1/auth/assurance/verify','post']]) paths[path][method].parameters=[{name:'organisation',in:'query',required:false,schema:{type:'string',format:'uuid'}},{name:'step_up',in:'query',required:false,schema:{type:'boolean',default:false}}]
paths['/management/runner/v1/mfa-authority']={post:{operationId:'current_instance_mfa_authority',security:[{instanceKey:[]}],requestBody:{required:true,content:{'application/json':{schema:{type:'object',additionalProperties:false,required:['organisation_id','instance_id','user_id'],properties:{organisation_id:{type:'string',format:'uuid'},instance_id:{type:'string',format:'uuid'},user_id:{type:'string',format:'uuid'},management_session_id:{type:['string','null'],format:'uuid'}}}}}},responses:{200:{description:'Exact current organisation policy and original current management session evidence',content:{'application/json':{schema:{type:'object',additionalProperties:false,required:['policy','user_id','instance_id','management_session_id','verified','verified_until'],properties:{policy:{$ref:'#/components/schemas/OrganisationMfaPolicy'},user_id:{type:'string',format:'uuid'},instance_id:{type:'string',format:'uuid'},management_session_id:{type:['string','null'],format:'uuid'},verified:{type:'boolean'},verified_until:{type:['string','null'],format:'date-time'}}}}}},401:{description:'Invalid current instance key or original session'},404:{description:'Actual org/instance/user access no longer available'},503:{description:'Current authority unavailable'}}}}
for(const [suffix,method,handler] of [['config','get','organisation_sso_configuration'],['start','post','start_organisation_sso'],['link/start','post','link_organisation_sso']]) {
  paths[`/api/v1/auth/organisations/{org}/oidc/${suffix}`] = {[method]:{
    operationId:handler,security:suffix==='link/start'?[{session:[]}]:[],
    parameters:[{name:'org',in:'path',required:true,schema:{type:'string',format:'uuid'}}],
    ...(method==='post'?{requestBody:{required:true,content:{'application/json':{schema:{$ref:'#/components/schemas/StartOidcAuthorizationRequest'}}}}}:{}),
    responses:{200:{description:'Organisation-bound SSO; signed group claims determine managed membership',content:{'application/json':{schema:{$ref:`#/components/schemas/${method==='get'?'OrganisationSsoPublic':'OidcAuthorization'}`}}}},401:{description:'Invalid or expired session'},404:{description:'Organisation SSO unavailable'},503:{description:'Identity provider unavailable'}}
  }}
}
for (const [route, request, response] of [
  ['execution-reserve', 'ExecutionAdmissionRequest', 'ExecutionAdmissionDecision'],
  ['execution-release', 'ExecutionReleaseRequest', 'ExecutionReleaseAcknowledgement'],
]) {
  paths[`/management/runner/v1/${route}`] = { post: {
    operationId: route.replaceAll('-', '_'), security: [{ instanceKey: [] }],
    requestBody: { required: true, content: { 'application/json': { schema: { $ref: `#/components/schemas/${request}` } } } },
    responses: { 200: { description: 'Authenticated reservation decision or exact completion acknowledgement', ...(response ? { content: { 'application/json': { schema: { $ref: `#/components/schemas/${response}` } } } } : {}) }, 401: {description:'Invalid instance credential'}, 404:{description:'Scope inaccessible'},409:{description:'Immutable binding or generation conflict'},422:{description:'Invalid request'},503:{description:'Execution admission unavailable'} },
  } }
}
const document = { openapi: '3.1.0', info: { title: 'LocalAI Cowork enterprise management', version: '3', description: 'Management metadata API. Application and device protocol remains v2; v3 workflow contracts do not imply enabled distributed execution.' }, paths, components: { securitySchemes: { session: { type: 'http', scheme: 'bearer' }, instanceKey: { type: 'apiKey', in: 'header', name: 'x-cowork-instance-key' } }, schemas } }
const bundle = { $schema: 'https://json-schema.org/draft/2020-12/schema', title: 'Enterprise v3 contracts', $defs: schemas }
const stable = value => Array.isArray(value) ? value.map(stable) : value && typeof value === 'object' ? Object.fromEntries(Object.keys(value).sort().map(key => [key, stable(value[key])])) : value
await mkdir(output, { recursive: true })
for (const [name, value] of Object.entries({ 'contracts.schema.json': bundle, 'openapi.json': document })) {
  const content = JSON.stringify(stable(value), null, 2) + '\n'
  const path = join(output, name)
  if (check) { if (await readFile(path, 'utf8') !== content) throw new Error(`${name} is stale`) }
  else await writeFile(path, content)
}
console.log(`enterprise_schemas=${Object.keys(schemas).length} enterprise_operations=${Object.values(paths).reduce((n, p) => n + Object.keys(p).length, 0)}`)
