# OIDC identity binding and upgrade behavior

Management supports separate operator-approved providers and signed group-to-role
mapping per organisation. The existing global provider remains available for
standalone workspace servers. Management rejects global-provider sign-in, linking
and callbacks, even when legacy OIDC environment variables are present. Explicit operator-owned provider assurance
mapping and opt-in SCIM/LDAPS directory synchronisation are implemented source
features. External-provider and installed Directory qualification remain open. This document
does not claim OIDC certification or production acceptance of assurance mapping.

## Organisation providers and setup

Complete local setup first: create the first local administrator, enroll and
verify two-factor authentication, save the recovery codes, then create the
organisation. The management API requires verified session assurance for that
platform administrator before the SSO settings can be accessed. SSO is off by
default; an organisation administrator must explicitly save an enabled, available
configuration before its sign-in link is published.

1. Configure a confidential OIDC client with exact redirect URI
   `https://cowork.example.com/api/v1/auth/oidc/callback`, authorization-code flow
   and S256 PKCE. Configure the IdP to put a bounded array of exact group strings
   into a signed ID token claim, `groups` by default. Groups from userinfo, browser
   fields or unsigned tokens are never used. Group-overage references or malformed
   claims do not grant a role.
2. Copy `deploy/enterprise/oidc-providers.example.json` to a private
   `providers.json`. Replace the organisation UUID, exact issuer, client ID and
   management public origin. Put each secret in a separately referenced file.
   A provider needs an explicit organisation allowlist; there is no wildcard.
   The tenant API accepts a provider ID, never URLs, secrets or host-file paths.
3. Set `COWORK_ORGANISATION_OIDC_PROVIDERS_FILE` on **management only**. The file
   and referenced secrets must be readable by its service user. For Compose, use
   `docker-compose.sso.yml` after the base Compose file and set
   `COWORK_ORGANISATION_SSO_DIR` to this private directory. It mounts only into the
   trusted management process at `/run/oidc`, never into instances or sandboxes.
   Docker Desktop: recreate the provisioner after management, since its current
   local profile shares management's network namespace.
4. For the management Helm chart, create an operator-owned Secret containing
   `providers.json` and the referenced secret keys, then set `sso.existingSecret`.
   Secret paths start with `/run/oidc/`. Set `sso.egress` to the explicit CIDRs and
   ports used by discovery, JWKS and token endpoints. Allowlist changing external
   IPs with the operator's network policy tooling; the chart has no allow-all rule.
   Restart all management replicas after changing the catalog or a secret, using
   the same catalog on every replica. Helm rendering is not Kubernetes acceptance.
5. As organisation administrator, open **Management → SSO**, choose an approved
   provider, enable admission, configure exact group-to-role mappings and choose
   whether matching identities may create accounts/memberships. Save after reading
   the effect on existing managed members. Copy the organisation sign-in link for
   manual delivery. The normal login form has no SSO configuration or organisation
   ID field; members use the organisation-specific sign-in link.
   Existing users sign in first and use **Security → Organisation ID for SSO →
   Link SSO identity**. The management SSO panel also offers account linking.

Discovery is lazy with a five-minute per-provider cache, bounded HTTP timeouts and
no redirects. An unreachable IdP does not prevent management from starting or
other providers from working. Invalid local catalog/secret files fail startup.
Only HTTPS is accepted apart from explicit loopback development addresses.

## Admission, changes and revocation

The verified `(issuer, subject)` owns the account. Automatic organisation admission
requires a matching signed group and `auto_provision`; new accounts also require a
verified email that is not already owned by an existing account. The strongest
matching role wins: organisation administrator, member, then audit reader. SSO
never grants platform administration, content-reader rights or instance assignment.
An existing manually assigned membership is preserved, even if the IdP has a
stronger role. An explicit administrator membership assignment converts an SSO
membership to a manual one; the source is visible in the members table.

At an organisation SSO login, absence of a matching group suspends an existing
SSO-managed membership and commits that revocation without issuing a login code.
Audit-only or suspended membership cannot access the execution/content broker.
Existing instance sessions are checked against current management permissions;
already executing actions are not claimed to be retroactively undone. A failed
audit write rolls back configuration, account, binding and membership changes.
Suspended members cannot bypass suspension by accepting another invitation.

Saving SSO settings increments the revision and suspends all SSO-managed members
until they sign in under the new rules. Manual memberships stay active. At least
one manually assigned, active organisation administrator must remain. Withdrawing
a provider, changing its security configuration or removing its organisation
allowlist disables affected SSO configurations at management startup and suspends
managed members. The denial and revision change are persisted, including for older
replicas during rollout. Reintroducing a provider requires the administrator to
review and enable SSO again; restoring a file alone does not restore memberships.
Secret rotation alone does not change role-policy fingerprints.

**Directory changes are discovered at the next scoped SSO login.** This version
has no SCIM, IdP event stream or periodic membership reconciliation. For immediate
central revocation, disable/change the organisation's SSO configuration or apply
an explicit membership change. Ordinary global sign-in does not refresh group
membership. Authentication grants no implicit access to another organisation.

Migration 0042 adds versioned organisation SSO policy and the `suspended` read-only
membership state. Existing manual memberships are unchanged. Back up the database,
authentication keys, provider catalog and client secrets before deploying.

## Account ownership

The verified issuer and subject identify an external account. An email claim,
including a verified email, never authorizes joining an existing Cowork account.
This follows [OpenID Connect Core, claim stability and uniqueness](https://openid.net/specs/openid-connect-core-1_0.html#ClaimStability).

On standalone workspace servers, with `COWORK_OIDC_AUTO_PROVISION=true`, a valid identity with a verified email can
create a new Cowork account. In the global-provider flow, the new account has no
password, platform role, organisation membership or instance access. The scoped
organisation flow additionally applies the admission rules above. Existing accounts must sign in using
their established method, then select **Security → Link SSO identity**. A subject
already bound to another account cannot be transferred through that flow. Changing
the provider's email claim does not change account ownership or the local email.

Linking requires the original authenticated session to remain active at callback
time. Logout, session expiry, account deletion, or expired mandatory administrator
verification prevent completion. For platform administrators, unvalidated IdP
`amr`/`acr` claims never satisfy the management second-factor requirement. A new
session returned by SSO receives provider verification only through the explicit
validated policy described below; otherwise it needs local verification.

## Explicit provider verification policy

The operator can copy `deploy/enterprise/oidc-assurance.example.json` into a private
configuration directory and replace its exact issuer, client ID and claim tuple.
The example is illustrative; verify the actual meaning and signed claims with the
selected provider before approving it. No generic `mfa`, `otp`, password, SSO login,
scope or group automatically counts as a verified factor.

For an organisation provider, put the complete policy object in its optional
`assurance` property in `providers.json`. `null` or absence preserves existing
login without provider verification and preserves the legacy provider fingerprint.
For the global provider, set `COWORK_OIDC_ASSURANCE_POLICY_FILE` to the policy file.
Its `issuer` and `client_id` must match the configured provider exactly. The file
is bounded to 128 KiB; on Unix it cannot have multiple links or group/other write
permission. A read-only Kubernetes Secret projection may point to a regular file
inside that same protected operator directory. Tenant APIs accept no assurance
claims, policy files, paths or provider credentials.

Each mapping specifies an exact, case-sensitive `(acr, amr)` tuple. `amr` is a
complete unordered set: every configured value must occur, extra values do not
match, and duplicates are rejected. `acr: null` means the signed token must have
no `acr`; an empty `amr` set permits an absent/empty `amr` only. A tuple needs a
nonzero `acr` or a nonempty `amr`. Matching either complete tuple in `mappings`
permits verification. This deliberate exact mapping requires reviewing provider
changes; unknown claims leave ordinary SSO login available without elevation.

Only the typed claims from the exact ID token already checked for signature,
issuer, audience, authorized party, expiry, nonce and optional access-token hash
are examined. `auth_time` is required for verification, cannot be in the future
or after `iat`, and must still be fresh. The request asks for `max_age` and mapped
`acr_values`; merely requesting them grants nothing. The verification deadline is
the earlier of signed token `exp` and `auth_time + max_age_seconds` (1–43200
seconds). A refreshed local access/refresh token never extends this deadline.
The meanings and freshness of these claims follow
[OpenID Connect Core](https://openid.net/specs/openid-connect-core-1_0.html#IDToken);
the operator remains responsible for the provider-specific mapping.

Migration 0062 adds separate nullable provenance to native one-use codes and
sessions; existing local verification and old sessions remain unchanged. Provenance
binds the original user/identity, policy fingerprint/revision/epoch and, for scoped
SSO, the organisation provider fingerprint and SSO revision. Code exchange, every
management session gate, and account-link completion check current policy authority,
identity ownership, expiry and scoped admission. Local verification retains its
existing twelve-hour lifetime and enrolled-factor checks. Provider verification
does not enrol a local authenticator or grant a role, content access or instance
assignment. The management gate shows a fresh approved SSO sign-in action when
available; Security shows current provider verification and its deadline.

An initial configured policy registers its stable scope (`global` or
`organisation:<provider-id>`) at revision 1. Restarting a process never re-enables
a disabled scope or overwrites an existing policy. Replicas with stale or different
catalogs provide no assurance when their fingerprint differs from the database.
Changes and re-enabling require these **local operator** commands, with the ordinary
private database environment; they are not HTTP/tenant APIs:

```sh
cowork-server oidc-assurance inspect global
cowork-server oidc-assurance disable global 1
cowork-server oidc-assurance activate global /run/oidc/global-assurance.json 2
cowork-server oidc-assurance inspect organisation:company
```

Each successful change uses a new revision and epoch, invalidating earlier code/
session assurance. Repeating the exact original request returns its original
immutable receipt; reusing that revision for another request conflicts. Receipt,
registry change and authentication audit are one transaction. To withdraw a global
mapping across replicas, **disable the scope before removing its local file**;
absence on one replica cannot revoke another replica's operator configuration.
To replace a policy, distribute the reviewed file/catalog, inspect and explicitly
activate its expected revision. Organisation provider fingerprint changes retain
the existing fail-closed SSO reconciliation: a manual administrator must review and
re-enable scoped SSO, and managed members sign in again under the new rules.

Compose organisation policies use the existing `docker-compose.sso.yml` read-only
operator directory. Its optional global environment fields can reference
`/run/oidc/global-client-secret` and `/run/oidc/global-assurance.json`; retain
`providers.json` (an empty array suffices for global-only use). For standalone
API Compose, add `deploy/docker-compose.oidc-assurance.yml` after the existing OIDC
override and set `COWORK_OIDC_ASSURANCE_POLICY_PATH` to the private policy file.
Existing OIDC installations do not need this opt-in override. Helm supports the
same global setup through `sso.global` and the already private `sso.existingSecret`,
with exact Secret key names and explicit existing SSO egress rules. These source
profiles have not been deployed or externally qualified by this change.

Identity resolution, binding, authorization-code issuance and authentication audit
are one database transaction. A failed audit write grants no new identity or code.
Concurrent first logins for one subject resolve to one account. Different subjects
presenting one existing email do not merge. Provider error details are not returned
to clients, and discovery/token requests have bounded timeouts and no redirect
following. Authentication state is consumed even on provider rejection.

## Migration 0041

Back up the database and authentication keys before upgrading. The migration adds
an explicit binding method and the session responsible for a pending link.

- Existing links with a matching `auth.oidc.link` audit event retain access.
- Accounts and identities created together by the old automatic-provisioning
  transaction retain access, identified by their unchanged transaction timestamps.
- Other old links are marked `legacy_unverified` and cannot sign in. Sessions for
  affected accounts are revoked and their outstanding native authorization codes
  removed. The identity record remains available for review and explicit repair.
- Previously pending linking operations are invalidated; start linking again from
  an active session. Ordinary pending sign-ins retain their original expiry.

Affected people sign in through an already-established password/passkey or another
proven identity, then explicitly link the intended SSO identity again. If no
established sign-in method is available, an operator must verify account ownership
through their account-recovery process. Matching the email alone is insufficient.
This is not an automated recovery or identity-transfer feature.

Inspect migration results without exposing subjects, emails or secrets:

```sql
SELECT binding_method, count(*) FROM oidc_identities GROUP BY binding_method;
SELECT count(*) FROM auth_sessions
WHERE revoke_reason='oidc_binding_requires_verification';
```

## Continuous directory provisioning (migration 0063)

The opt-in Management Directory area supplies SCIM 2.0 inbound provisioning and
read-only LDAPS/RFC2696 paged polling. These are separately registered original
connector UUIDs bound to one organisation, one approved SSO provider, exact issuer
and client ID. The immutable SCIM `externalId`, or configured immutable LDAP
subject attribute, must equal that provider's OIDC subject. LDAP group names map
through the organisation's existing exact SSO group mappings. They never grant
platform administration or content-reader rights. LDAP subject attributes must
be textual; binary AD `objectGUID` is not silently interpreted. Group member DNs
must match the canonical DNs returned for Users in the same complete cycle.

Copy and review `deploy/enterprise/directory-connectors.example.json` into a
private operator directory as `connectors.json`. Replace all example UUIDs,
provider identities, endpoints, filters and immutable subject mappings. Supply
private token/password/CA files at the configured paths. SCIM tokens need at least
32 printable random bytes. On Unix files are regular, single-link, mode0600 and
their parent is not writable by others; Docker bind files must satisfy those
permissions inside the container. Use the opt-in
`deploy/enterprise/docker-compose.directory.yml` overlay, or set
`COWORK_ORGANISATION_DIRECTORY_FILE` directly. Management already requires its
authentication master key and audit key. Credentials, profiles, original replies
and worker cursors use a separate per-connector envelope key domain; custom CA
bytes and credential hashes are included in the operator fingerprint.
The supplied server image runs as UID/GID10001; the private directory and mode0600
files must be readable by that exact runtime identity (for example ownership
10001:10001 and directory mode0700). The overlay does not change host permissions.

Startup only inserts an absent exact connector. Restart does not reactivate a
disabled slot or overwrite a changed binding. Inspect and explicitly change the
current operator revision using the configured private catalog:

```sh
cowork-server directory inspect CONNECTOR_UUID
cowork-server directory disable CONNECTOR_UUID EXPECTED_REVISION
cowork-server directory activate CONNECTOR_UUID EXPECTED_REVISION
```

Changing the original organisation/provider/issuer/client/kind requires a new
connector UUID. Operator changes retain their immutable original receipts and
rotate epochs. Other API pods without a catalog do not revoke foreign slots.
In Management, select Preview first, save account-provisioning and same-provider
membership-adoption preferences, then review the exact legacy inventory. Preview
stores only staged source facts; it creates no accounts or memberships. Existing
accounts associate only through a verified exact issuer+subject binding, never
email. Initial new accounts have no password or platform-admin flag; an existing
email causes a conflict. Missing LDAP/email profiles use a unique `.invalid`
account address. Local/foreign-provider accounts and manual/content-reader
memberships are preserved.

Migration0063 adds nullable login origins without backfill or automatic logout.
The separate ten-minute containment plan lists only users actually bound to the
selected provider and concrete organisation/source. Confirmation checks the full
inventory hash, current policy/SSO/connector revisions and explicit acknowledgement.
It logs out all listed users' unclassified old sessions/codes, which can include
local old sign-ins. The UI calls this user-wide legacy containment, not selective
connector revocation. Classified local/foreign sessions survive. Exact user-row
locks and insert guards prevent old replicas creating unknown sessions after
confirmation; old signed-group writers cannot promote directory-managed roles.
No role/session fallback is admitted on a guard error. Active sync then requires
fresh source input for its new epoch; a saved preview roster never becomes a grant.

SCIM base URL is the Management HTTPS origin plus
`/management/scim/v2/CONNECTOR_UUID`; the production Management gateway routes
that exact prefix and instance gateways still reject Management routes. Use only
the connector bearer. Users and Groups support POST/GET/PUT/PATCH/DELETE, exact
`externalId eq`, `userName eq`/`displayName eq` filters, startIndex/count pages
(count0..100), ETags and discovery. Updates/deletes require the returned If-Match;
Idempotency-Key is supported and binds the exact original request. Bulk, password
provisioning, sorting, arbitrary filter expressions and cross-connector member IDs
are rejected. PATCH supports core names/profile fields, active and group member
add/replace/remove (including `members[value eq "UUID"]` removal). External IDs
and tombstones cannot be reassigned. Repeat mutations return their original
encrypted response; after response retention expires the once tombstone prevents
reexecution rather than inventing a current reply.

LDAP performs verified LDAPS simplebind, never plaintext remote LDAP, referrals
or remote mutations. Its owned transport bounds BER frames before the library
parser (512KiB/frame, 16MiB/page,1002messages, depth32), page size1..500,
operation deadlines, leased/cas-bound encrypted cursors and bounded backoff. A
complete Users/Groups cycle rechecks a missing immutable subject individually
before retirement. Partial scans/failures never prove removal. Changed policy,
SSO/configuration or epochs fence work after every external operation. Disabling
identities/connectors revokes their actual new-origin sessions/codes and withdraws
only their contributions; manual/other-source contributions survive. Deleted
profile/reply ciphertext is purged in bounded batches after configured retention;
identity/tombstone/once-receipt and audit provenance remain.

This source addition has no real Directory/SCIM provider, mixed-replica migration,
installed-image or production acceptance yet. Runtime provider configuration and
compatibility (including paged-cookie restart support and canonical DN format)
require operator qualification. Broad organisation MFA policy is separate work.

## Acceptance

`scripts/test-enterprise-oidc.py` starts a disposable PostgreSQL cluster, the actual
server binary and an in-process HTTP provider with a newly generated RSA key. It
exercises discovery/JWKS, RS256 signatures, issuer/audience/authorized-party checks,
nonce, token expiry, access-token hash, PKCE, one-use state, email collision,
concurrent provisioning/linking, session/MFA revocation, and transactional audit
failure. The provider is a test double; signature validation and application
authorization are real. This is not acceptance of Entra ID, Keycloak or another
external deployment.

```sh
python3 scripts/test-enterprise-oidc.py --server target/release/cowork-server
python3 scripts/test-enterprise-oidc.py --server target/release/cowork-server \
  --previous-server /path/to/pre-0041/cowork-server
```

The second command exercises the old server's actual email matching, initial
account creation and explicit linking, then upgrades the same database and checks
preservation, revocation and explicit repair. It requires a pre-0041 binary with
management MFA support. Tests require Python 3, OpenSSL and PostgreSQL 16 binaries;
they use only disposable loopback services and retain no credentials on completion.

Organisation-specific acceptance uses two independent signed test providers:

```sh
python3 scripts/test-enterprise-oidc.py --server target/release/cowork-server \
  --organisation-sso
```

It exercises exact group mapping, cross-organisation rejection, email collision,
role downgrade/removal against existing sessions and the real instance broker,
manual-role preservation, suspended invitations, explicit linking after logout,
revision-bound callbacks, disabled admission, catalog withdrawal/restoration and
transactional audit failures. The browser/desktop component and runtime tests cover
scoped routes, server-bound callback restoration, unavailable providers and revision
conflicts. A real external IdP and Kubernetes failover remain unqualified.
