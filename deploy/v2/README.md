# Selfhosting alpha2

For the new Kubernetes-native execution path, use
[Agent Sandbox architecture](../../docs/AGENT-SANDBOX-ARCHITECTURE.md) and
[operator workflow](../../docs/AGENT-SANDBOX-OPERATIONS.md). Its separate
`agent_sandbox.py` renderer replaces the host Docker dependency for that topology;
the older deployment workflows below retain their existing behavior.

Current lifecycle addition: [team workspace backup and restore](../../docs/TEAM-WORKSPACE-BACKUP.md)
includes separately mounted Compose/Kubernetes tool workspaces, exact retained-
record Docker quiescence and explicit retention of newer files. This source
addition has bounded private fixture checks; it does not qualify a live cluster.

Current image builds write `/opt/cowork/sbom.spdx.json` before operator keys or
data volumes exist. It binds the actual copied daemon, original Rust/npm lock
graphs, installed Debian package versions/ownership, `/usr` software hashes,
native PE/ELF library imports and the private Python venv's METADATA/RECORDs.
Inventory does not execute those programs or read `/etc` secrets/home/user data.
Required external OS imports and opaque upstream static dependencies stay
explicit; this source addition has not run a new image build or HA acceptance.

These files build the actual daemon and web frontend from this repository and
define one API, one worker, PostgreSQL, a TLS proxy and optional operator-owned
S3. They are an installation basis; no containers, Kubernetes resources or VMs
were started to produce these files. No multi-host HA or unattended restore is
claimed. API/worker share the same private data/key volume and UID. PostgreSQL
dispatch and native order locks fence concurrent admissions; keep the supplied
single API/single worker topology until qualifying a larger deployment.

Use a Linux Docker Compose host, an explicit DNS hostname pointing to it and
reachable ports80/443. HTTPS is terminated by Caddy. The daemon port/database/S3
are not published. Set `COWORK_IMAGE` and the base image arguments to your selected
image tags/digests when creating a release. The default source-built image also
contains Python Office/PDF libraries, LibreOffice, Git, gh, Chromium and a Python
venv with pinned Playwright for navigation/DOM/screenshots/click/fill. Native
tools retain the daemon owner's OS rights inside that container; they are not
tenant isolation. Chromium sandbox operation also depends on the host's permitted
user namespaces; use a separate qualified browser/container runner for untrusted
tools. No Docker socket is mounted into the daemon.

From the source checkout, initialize a **new** private deployment directory owned
by the same ordinary Linux UID that runs Docker (typically1000):

```sh
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork init --site cowork.example.com
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork build
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork start
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork status
```

`init` generates private database/S3 secrets and a UUID-bound Compose project
marker. Startup generates separate private owner/native authorities, identity
bootstrap/settlement secrets and the independent encrypted-team-key master.
Preserve the whole `data/` directory. Read only the bootstrap/operator file you
need on the server, then manually initialize team identity and configure LiteLLM
through the Teams UI. Those keys are never served from the frontend directory.
`data/toolbox.json` optionally selects absolute executable aliases and a private
GitHub token file. Provider credentials/configurations are explicit operator
inputs; they are not discovered from existing user profiles.

Remote team MCP connections require `COWORK_TEAM_MCP_ALLOWED_ORIGINS`, a
comma-separated list of exact origins such as `https://mcp.example.com`.
The empty default permits no remote MCP connection. Set the same list for API
and worker in the Compose `.env`, or in the Kubernetes operator environment.
Credentials and individual server/tool grants are configured separately in Teams.

For an existing internal LiteLLM service using plain HTTP, explicitly set
`COWORK_TEAM_LITELLM_HTTP_ORIGINS=http://litellm:4000` (or its actual exact origin)
for both API and worker. This is an operator allowance for the configured team
provider endpoint. The empty default rejects non-loopback plain HTTP; HTTPS
requires no HTTP exception. Set the corresponding Kubernetes environment entry
or HA operator ConfigMap when using that topology.

External S3 uses `COWORK_S3_ENDPOINT`, `COWORK_S3_BUCKET`,
`COWORK_S3_REGION=us-east-1`, and separate private
`COWORK_S3_ACCESS_KEY_ID_FILE`/`COWORK_S3_SECRET_ACCESS_KEY_FILE` mounts in an
operator Compose override. File values are mutually exclusive with direct key
environment variables. HTTPS is required by default except explicit loopback.
Without an endpoint/bucket, the managed local fallback stores files under the
private data directory.

For the optional local MinIO service, initialize with `--local-s3`. The overlay
deliberately sets `COWORK_S3_ALLOW_INSECURE_HTTP=1` for the startup-pinned internal
`http://minio:9000`; no S3 port is exposed. It creates the `cowork` bucket and uses
separate0600 operator files for both daemon processes. This is a conscious private
network HTTP option rather than a change to the public HTTPS default. MinIO's
community repository is now archived and source-only, so the optional
`Dockerfile.minio` builds the fixed legacy source release and client into an
operator-owned image. Supply your maintained compatible image/fork instead for
your own deployment policy. [Official MinIO source status](https://github.com/minio/minio).

Backups require the installed `age` CLI and your explicit public recipient. The
script stops this UUID-owned stack's writers, makes a logical PostgreSQL dump,
captures private configuration/key custody and local MinIO bytes, seals a file
manifest, encrypts the whole archive and resumes the previously running services.
Plaintext exists only in a private temporary directory during that command.
Keep the private age identity elsewhere; it is not included in the backup.
External S3 objects need the operator's own object/version backup policy; the
local archive explicitly records that those objects are not captured.

```sh
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork backup --recipient age1YOUR_PUBLIC_RECIPIENT
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork-restored restore \
  --input /srv/localai-cowork/backups/SELECTED.tar.age \
  --identity /secure/age-identity.txt --site cowork.example.com \
  --image localai-cowork-v2:YOUR_RELEASE
```

Restore requires a new absent destination and verifies all decrypted members,
sizes and SHA256 before importing the logical database. It rejects symlinks,
hard links, duplicate names, traversal and unknown archive members. Do not run
the original and restored identity installation against the same database.

`update` preserves an encrypted pre-update checkpoint, keeps writers quiescent,
then recreates only the selected API/worker image. A failed update leaves its
checkpoint available; older binaries must not be run blindly against a newer
schema. Explicit rollback restores the captured owned database/schema and key/data
snapshot before restarting the prior image. Newer files are retained separately,
and writes after the checkpoint are not part of the restored database.

```sh
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork update \
  --image localai-cowork-v2:NEXT_RELEASE --recipient age1YOUR_PUBLIC_RECIPIENT
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork rollback \
  --checkpoint /srv/localai-cowork/backups/update-SELECTED.json \
  --identity /secure/age-identity.txt --restore-database
```

For an existing Kubernetes cluster, first push your built image to your own
registry. The template supplies one Recreate deployment with API/worker in the
same pod/private PVC, one PostgreSQL PVC/service and an Ingress. Existing storage
class, ingress controller and the `cowork-tls` TLS secret are operator inputs.
It does not provision a VM/cluster or certificate authority. Render explicitly:

```sh
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork render-kubernetes \
  --namespace localai-cowork --output /srv/localai-cowork/kubernetes-rendered.yaml
kubectl create namespace localai-cowork
kubectl -n localai-cowork create secret generic cowork-database \
  --from-file=postgres_password=/srv/localai-cowork/secrets/postgres_password
# Create your selected TLS secret in that namespace, then:
kubectl apply -f /srv/localai-cowork/kubernetes-rendered.yaml
```

For Kubernetes S3, mount explicitly selected key files and copy them into a
0600 same-UID private data leaf in an operator init step; projected group-readable
Secret files do not satisfy native S3 custody validation. Kubernetes backups
require a matching quiesced operator procedure, PostgreSQL dump, PVC data/key
snapshot and object-storage snapshot. The Compose lifecycle commands are scoped
to their own Compose deployment and do not operate on Kubernetes resources.

To select the operator-installed `runsc` runtime for the API/worker containers,
initialize with `--gvisor` (also available for a fresh restore). That chooses
`compose.gvisor.yml`. It does not install gVisor or assert host compatibility.
Kubernetes supplies `kubernetes.gvisor-patch.yaml` for an existing `gvisor`
RuntimeClass; apply that explicit patch in your selected namespace.

Team tools use a separate image and an explicit Docker-control overlay. Build it
from the checkout, publish it to your own registry, and select the resulting
immutable `name@sha256:digest`. `Dockerfile.tools` contains only Python,
Playwright/Chromium, Office/PDF adapters, Node, Git and uncredentialed gh, plus
the strict guest entrypoint. It contains no daemon, database or operator keys.

```sh
docker build -f deploy/v2/Dockerfile.tools -t YOUR_REGISTRY/cowork-tools:YOUR_RELEASE .
# Push to your own registry if used, then inspect the exact resulting digest.
# Create a private UID1000 directory at the selected Docker-host root first.
```

For Compose, `configure-team-tools` explicitly selects `compose.team-tools.yml`
for this deployment's subsequent lifecycle commands. It sets
`COWORK_TEAM_TOOL_IMAGE`, `COWORK_TEAM_TOOL_HOST_ROOT` (the
absolute host directory backing only `toolbox/workspaces`), and
`COWORK_DOCKER_GID` for the selected socket. The overlay gives Docker authority
only to the trusted worker. Both API and worker mount the same selected workspace
directory and receive the same pinned tool configuration so admitted team-file
access sees the actual guest outputs. The API receives no socket or socket GID.
The guest receives only its exact order UUID directory at `/workspace`, fixed
program aliases and a strict action JSON, never the socket, daemon keys, global
GitHub token, host profiles or database. Docker socket access is operator host
authority; it is not tenant authority. Do not mount it into API or tool guests.
Configure it before the first worker start; choose a fresh private UID1000
workspace directory instead of hiding an existing populated directory:

```sh
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork configure-team-tools \
  --host-root /srv/cowork-team-workspaces \
  --image YOUR_REGISTRY/cowork-tools@sha256:YOUR_EXACT_DIGEST \
  --docker-gid YOUR_SOCKET_GID
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork start
```

Team guest network defaults to `none`. Browser/GitHub network operations require
an operator-created Docker network with suitable egress rules, explicitly named
in `COWORK_TEAM_TOOL_NETWORK`; `host` is rejected. Setting
`COWORK_TEAM_TOOL_RUNTIME=runsc` selects an already registered runtime for those
guests. Docker/gVisor setup and network policy belong to the operator.

Team GitHub uses its team's encrypted credential and fixed hostname only after
approval of the exact action and credential revision. The trusted worker creates
a private credential environment outside the workspace. Only a GitHub guest
receives it. The guest runs remote API management from a fresh `/tmp` directory,
rejects host/header overrides and local git operations, and redacts the exact
token from both output streams before forwarding them. Operator GitHub tokens
are never forwarded. Clone, checkout, sync and workspace Git hooks are excluded
from this authenticated adapter.

`kubernetes.ha.yaml` is an alternative topology with two API replicas, two worker
replicas and separate disruption budgets. Render it with `--ha`, explicit
`--database-host`, `--s3-endpoint`, `--s3-bucket`, `--s3-region`, `--tool-image`
and `--docker-gid`. It assumes external HA PostgreSQL with a trusted CA and
HTTPS S3, not the supplied single PostgreSQL service. Create `cowork-ha-keys`
with `postgres_password`, `postgres_ca.pem`, `s3_access_key` and `s3_secret_key`;
the init step copies only these selected projected files into private same-UID
regular files. Preserve an already initialized installation's private key/data
directory in the RWX PVC. Do not initialize several unrelated installations
or roll out incompatible schemas together.

The template fixes storage class `rook-cephfs` and requires selected nodes to
carry `cowork.local/shared-workspaces-ready=true`; workers additionally require
`cowork.local/docker-tools-ready=true`. Before labeling those specific nodes,
the operator must mount the **same shared RWX tool-workspace filesystem** at
`/srv/cowork-team-workspaces` on each one, UID1000/private permissions, and
provide the chosen Docker socket/GID on worker nodes. A PVC path inside a pod
alone is not a Docker-host bind path. Cluster-wide locks, atomic rename, fsync,
same physical custody files and broker semantics must be established for that
exact storage deployment. Generic NFS paths are not accepted as equivalent.
The standard single topology remains the default. This manifest does not
constitute a load, takeover or failure-recovery qualification.

`kubernetes.ha.gvisor-patch.yaml` separately selects API RuntimeClass and worker
guest `runsc`; use it only when that combination is supported by your selected
nodes. A Kubernetes CRI based on containerd alone does not provide Docker's
socket API. The operator must explicitly provision the worker Docker-control
service if choosing this adapter.


Alpha.4 stellt optionale Betreiberwerte fÃ¼r Passkeys, FCM und Fleet Ã¼ber die
Compose-Umgebung und beide Kubernetesvorlagen bereit. Einrichtung und private
Dateipfade stehen in `docs/INSTALLATION-ALPHA4.md`. `lifecycle.py` erhÃ¤lt diese
Werte bei eigenen Aktualisierungen. Der Betreiber stellt die privaten Dateien
unter dem tatsÃ¤chlichen `COWORK_DATA_DIR` bereit, mit Daemonbesitzer und Modus
0600; eine leere Vorgabe lÃ¤sst den jeweiligen Dienst unkonfiguriert.

Die aktuellen Quellen erweitern den optionalen privaten Android-Runner um
interaktive Terminals und projektgebundene GitHub-ZugÃ¤nge. Die Vorlage
`android-private-runner.example.json` wÃ¤hlt `terminal_program: "bash"` aus der
konkreten Programmliste. `github: false` lÃ¤sst den GitHub-Gast ausgeschaltet;
der Betreiber aktiviert ihn ausdrÃ¼cklich und der Benutzer hinterlegt danach
seinen eigenen projektgebundenen Zugang im Konto-/Privatbereich. API und Worker
benÃ¶tigen dieselbe unverÃ¤nderliche Runnerbindung. Nur der Worker erhÃ¤lt den
ausgewÃ¤hlten Docker-Socket; die GÃ¤ste verwenden `runsc` und `network=none`.
Das aktuelle `Dockerfile.tools` enthÃ¤lt beide neuen GÃ¤ste. Das Tools-Image muss
neu gebaut und sein tatsÃ¤chlich beobachteter Digest in der privaten Konfiguration
eingetragen werden. Alte Images enthalten diese Quellen nicht automatisch.

Die Quellen mit Migration 0066 binden private Werkzeuge zusÃ¤tzlich an einen
dauerhaften Host-/Engine-Slot. Ein API-Pod ohne eigenen Runner deaktiviert keine
anderen Slots. Ein Neustart hebt einen ausdrÃ¼cklichen Widerruf nicht auf und
ersetzt keine abweichende aktive Konfiguration. Nach einer Ã„nderung der
Runnerkonfiguration prÃ¼ft der Betreiber den eigenen Slot und aktiviert die
gewÃ¼nschte Bindung ausdrÃ¼cklich mit der beobachteten Revision:

```sh
cowork-daemon private-runner inspect --configured
cowork-daemon private-runner activate --configured EXPECTED_REVISION
cowork-daemon private-runner revoke --configured EXPECTED_REVISION
```

`--configured` liest dieselbe private `COWORK_ANDROID_PRIVATE_RUNNER_CONFIG`
wie der normale Start. Alternativ akzeptieren die Befehle eine OS-private
Datei mit der vollstÃ¤ndigen ursprÃ¼nglichen Runnerbindung. Die Befehle starten
keine GÃ¤ste. Aktivierung und Widerruf speichern ihre Originalquittung atomar;
derselbe ursprÃ¼ngliche Aufruf liefert diese Quittung auch nach verlorenem ACK,
ohne eine spÃ¤tere Revision erneut zu verÃ¤ndern. Vorhandene LÃ¤ufe Ã¼bernehmen
keine neue Runnerrevision. TatsÃ¤chlicher Gastabschluss und Dateipromotion
brauchen weiterhin ihre eigenen ursprÃ¼nglichen physischen Belege.

FÃ¼r den neuen historischen Organisationsimport wird der Installationsspeicher
ausdrÃ¼cklich konfiguriert, beispielsweise mit
`cowork-daemon historical-import-storage 0 34359738368` fÃ¼r eine neue
32-GiB-Policy. Ã„nderungen verwenden die tatsÃ¤chlich gelesene Policyrevision.
Die aktuelle Speicherbelegung begrenzt Verkleinerungen; derselbe ursprÃ¼ngliche
Konfigurationsvorgang behÃ¤lt seine Originalantwort. Organisationsquoten gelten
zusÃ¤tzlich. Die API liegt unter
`/api/v2/historical-import/{team}/{project}` und verwendet eine echte
Benutzersitzung mit aktuellen Inhaltsrechten.

PersÃ¶nliche native Importe verwenden `/api/v2/native/historical-import` mit
beiden nativen Authorities und der tatsÃ¤chlich eingerichteten OS-SchlÃ¼sselverwaltung.
Ihre Speicherpolicy wird im nativen Importbereich bestÃ¤tigt. Der Daemon hÃ¤lt
seinen privaten `historical-import-staging`-Ordner bis zum Abschluss seiner
OriginalempfÃ¤nger. Quellen, Vorschau und ausdrÃ¼ckliche Aktivierung sind getrennte
Schritte; die Dateistufe prÃ¼ft den ursprÃ¼nglichen nativen beziehungsweise
S3-Inhalt tatsÃ¤chlich zurÃ¼ck. Ein ungeklÃ¤rter Dateivorgang wird nicht neu gesendet.
Diese Quellenbeschreibung ersetzt keinen aktuellen Image-, Installations- oder
Produktionsnachweis.


### Actor-private Workflow and Crew installation / PersÃ¶nliche Workflows und Crews

The API/worker installation exposes `workflow.pure` only after the operator has
explicitly activated its current engine configuration. `crew.python` additionally
requires the exact registered private runner and the separately built **Crew
image**, not the ordinary `Dockerfile.tools` image. No package is installed at
runtime; an unavailable or changed distribution refuses admission.

For the Crew layer, prepare a complete offline Python wheel closure compatible
with the Python version in your tools image. Put all wheels in `crew-wheels/` at
the build-context root and all exact dependency versions and wheel SHA-256 pins
in `crew-requirements.lock` (pip `--require-hashes` format). Select an explicit
CrewAI version whose API passes the following qualification; no unpinned latest
version or model/provider credential belongs in this build context. Run the wheel
download on Linux with the same Python/platform as the selected tools image (or
provide pip's explicit target-platform/Python flags). The pinning helper only
reads original wheel metadata/bytes; the subsequent offline image build verifies
the complete dependency closure. Neither helper installs, activates or overwrites
an existing file. Windows operators must apply the existing same-UID private ACL
policy before copying the output to their Linux operator installation.

```sh
python3 -m pip download --only-binary=:all: --dest crew-wheels "crewai==OPERATOR_SELECTED_VERSION"
python3 tools/private_crew_setup.py lock --wheels crew-wheels \
  --crewai-version OPERATOR_SELECTED_VERSION --output crew-requirements.lock
docker build -f deploy/v2/Dockerfile.crew \
  --build-arg PRIVATE_TOOLS_IMAGE=YOUR_REGISTRY/cowork-tools@sha256:EXACT_TOOLS_DIGEST \
  -t YOUR_REGISTRY/cowork-crew:YOUR_RELEASE .
# Publish to your selected registry if needed, then obtain its immutable digest.
docker run --rm --network=none --runtime=runsc \
  --entrypoint=/opt/cowork/python/bin/python \
  YOUR_REGISTRY/cowork-crew@sha256:EXACT_CREW_DIGEST \
  -I /opt/cowork-private-tools/crew.py --inventory > crew-inventory.json
python3 tools/private_crew_setup.py configure --inventory crew-inventory.json \
  --image YOUR_REGISTRY/cowork-crew@sha256:EXACT_CREW_DIGEST \
  --output /srv/localai-cowork/data/private-crew.json
sha256sum deploy/v2/android-private-crew.py
```

The inventory emits `{"version":"...","source_sha256":"..."}` from the actual
installed CrewAI source/distribution bytes. The final command hashes the actual
bridge supplied by this release. Put these observed values in a same-UID private
operator file (0600), for example:

```json
{
  "format": 1,
  "image": "YOUR_REGISTRY/cowork-crew@sha256:EXACT_CREW_DIGEST",
  "crewai_version": "OBSERVED_INSTALLED_VERSION",
  "crewai_source_sha256": "OBSERVED_64_LOWER_HEX_SOURCE_SHA256",
  "bridge_sha256": "OBSERVED_64_LOWER_HEX_BRIDGE_SHA256"
}
```

Set `LOCALAI_ANDROID_PRIVATE_CREW_CONFIG` to that absolute private file in both
API and worker. The private runner's `image` must select the **same exact Crew
image digest**; `COWORK_ANDROID_PRIVATE_RUNNER_CONFIG` continues to identify its
own private runner file. Keep gVisor, `network=none`, the original host/engine
slot and the existing explicit programs/Office/browser/GitHub policy. Credentials
and broad host mounts are never passed to the Crew child.

On first startup, each optional engine is registered inactive. The following CLI
reads the same configuration and installation, without activating on inspection.
Use the revision returned by `inspect` and a new original UUID per change; repeat
the same UUID only to retrieve that exact original operation after response loss.
After installing a changed source/image/configuration, inspect and explicitly
activate its current configuration with CAS again. Restart alone never revives a
revoked engine or adopts a changed image.

```sh
cowork-daemon private-runner inspect --configured
cowork-daemon private-runner activate --configured EXPECTED_RUNNER_REVISION
cowork-daemon private-workflow inspect workflow
cowork-daemon private-workflow activate workflow EXPECTED_ENGINE_REVISION ORIGINAL_REQUEST_UUID
cowork-daemon private-workflow inspect crew
cowork-daemon private-workflow activate crew EXPECTED_ENGINE_REVISION ORIGINAL_REQUEST_UUID
# Explicit revocation of the same installation/engine:
cowork-daemon private-workflow revoke crew EXPECTED_ENGINE_REVISION ORIGINAL_REQUEST_UUID
```

In the personal workspace / im persÃ¶nlichen Arbeitsbereich, create the registry
entry, retain each immutable definition version, and explicitly publish the
chosen current version with its registry revision. The DE/EN task/schedule
selector binds `runner: crew|workflow`, `crew_id|workflow_id` and
`workflow_version_id`; old synced definitions and forked drafts grant no execution.
Workflow folder rights use the symbolic `private-run` folder, rebound only to
that actual run's single owned workspace; native folder IDs are never adopted.
Each start/occurrence rechecks the actual Actor/session/project, released task,
publication, every role/model profile, selected MCP metadata/configuration,
runner/engine epoch, original clock, grants and limits. Native-owner workflows
and credentials remain separate.

Registry POST/version/publication operations retain a request UUID and original
request SHA. After a lost response, use the original operation GET rather than
creating another request. The UI retains that original receipt without persisting
secret tokens or the definition text. `/api/v1/runs/{run}/workflow/preflight`
returns the actual initialized graph/current fences; `ready` describes graph
state, not a claim of physical qualification. Node pages use a real cursor and a
50-row limit. Full original node JSON is available by the listed artifact ID and
size/SHA; large dependency results are read explicitly in bounded pages. The
existing recovery panel requires fresh current continuation authority and all
original graph, transport and physical receipts before resuming the **same run**.

Operator release qualification still requires an actual Linux/runsc Crew model
and tool execution, parallel/delegated children, cancellation/acknowledgement loss,
original cleanup and scheduled occurrence on the selected image/runtime. Source
compilation, the inventory command and a metadata activation are not that runtime
qualification. Installations must supply their own model/MCP credentials and
actual image/runtime; no live deployment or key adoption was performed here.


### Original-host retirement / Ursprünglichen Host stilllegen (0075)

The lost-host path is an explicit operator setup, not Node-absence detection.
Before the first team guest is created, obtain the **actual manager identity**
using the separate server's `scripts/enterprise-manager-host-identity.py`. That
producer reads the current original `management_instances.execution_epoch` over
pinned HTTPS with the enrolled host operator credential, observes the original
owned VM/daemon, and signs its bare JSON in `OUTPUT.signature.json`. Keep the
independently enrolled RSA3072–8192 public key and source release pins outside
all guests. The backend activation receipt alone has no execution epoch and is
insufficient. Protected/adopted/version-1 controller VMs are ineligible.

Build a private profile from those originals, the actual owner-private capacity
ledger, and the deployed controller source release. `--team-id` identifies a
real existing V2 team; its UUID may differ from the original manager organisation.
The mapping is deliberate operator configuration, not a tenant identity import.
For a Docker-backed Kubernetes worker, also pass its current owner-private
lifecycle `--node-map` and `--node-name`; no guest label supplies a node UID.

```sh
python tools/team_lost_host/enroll.py \
  --identity /private/manager-identity.json \
  --signature /private/manager-identity.json.signature.json \
  --backend-activation /private/BACKEND-ACTIVATION-RECEIPT.json \
  --capacity-ledger /private/actual-capacity-ledger.json \
  --enrolled-public-key /private/enrolled-parent-controller-public.pem \
  --controller-source-root /absolute/deployed/Open_Cowork \
  --manager-producer /absolute/deployed/Open_Cowork/scripts/enterprise-manager-host-identity.py \
  --profile-id ACTUAL_NEW_PROFILE_UUID --team-id ACTUAL_V2_TEAM_UUID \
  --output /private/new-host-trust.json
# Same COWORK_TEAM_TOOL_IMAGE/HOST_ROOT and actual host/node environment as the worker.
# inspect performs positive DockerInfo and does not register or activate anything.
cowork-daemon host-trust inspect /private/new-host-trust.json 0 ORIGINAL_INSPECT_UUID
cowork-daemon host-trust activate /private/new-host-trust.json 0 ORIGINAL_ACTIVATE_UUID
# Read the returned revision for a later explicit revoke. Restart never activates.
cowork-daemon host-trust revoke /private/new-host-trust.json ACTUAL_REVISION ORIGINAL_REVOKE_UUID
```

The CLI checks that the actual Docker engine ID equals the originally observed
manager/backend VM daemon ID. A current local engine cannot be assigned to an
unrelated retired VM through a profile alias. Unsettled unqualified attempts
block late enrollment; existing records never acquire trust from this command.
New qualified guests persist the full source, approval, lease and trust profile
before the once-only Create dispatch, then capture their actual ID before Start.
Both Start and persistent browser calls recheck the current original host profile.

In the team's run recovery view, open **Recover a lost original host**, read the
complete original plan, review all affected runs and explicitly prepare the
host-wide intent. Current organisation admin/owner and Execute access to every
affected project, plus quiescent original leases, are required. Preparation fences
new starts on that original host. Download the issued immutable intent; do not
send the read-only preview with its nil request UUID to the controller. If the
HTTP response is lost, use **Read original admission / receipt only** and the
same saved operation; no VM action is dispatched by these V2 endpoints.

On the independent parent controller, separately use the existing witness tool
with its original VM root and original backend receipt:

```sh
python scripts/enterprise-host-retirement-witness.py plan \
  --vm-root /private/actual-original-vm-root \
  --intent /private/original-host-intent.json \
  --backend-receipt /private/BACKEND-ACTIVATION-RECEIPT.json \
  --output /private/new-retirement-plan.json
python scripts/enterprise-host-retirement-witness.py retire \
  --plan /private/new-retirement-plan.json \
  --signing-key /private/original-parent-signing-key.pem \
  --confirmation RETIRE_EXACT_ORIGINAL_STOPPED_VM \
  --output /private/original-retirement-witness.json
```

The original VM must already be positively stopped by the actual controller.
Its QMP/process/fd/mount/inode/deletion witness is checked; lease expiry, Docker
connection loss, a replacement node or a caller boolean cannot replace it.
Upload the original signed witness (at most 2 MiB) in the run recovery view.
Only the independently pre-enrolled key and exact original intent/allocation/
source pins are accepted. One original witness closes the recorded attempts
from that entire original VM; it cannot mint Docker `container_removed`, settle
unknown tool effects, replay tools or authorize workspace promotion. Original
records and receipts are archived before active metadata is removed. If the
control volume was lost, the pre-Create/current-ID journal restores **metadata
only**, never lost workspace bytes or guest results. The original SQL physical
fence persists until positive original evidence and all metadata closures exist.

DE: Betreiberregistrierung erfolgt **vor dem ersten Create**, mit echtem
Management-Export, ursprünglicher Allokation und getrennt registriertem
Controller-Schlüssel. Im Wiederherstellungsbereich werden vollständige
Originalbindungen und alle betroffenen Läufe ausdrücklich geprüft. Der
unabhängige Betreiber legt die tatsächlich gestoppte eigene VM still; V2 nimmt
nur deren signierten Originalbeleg an. Alte ungebundene Daten, nicht qualifizierte
physische/cloudbasierte Hosts und reine Node-Abwesenheit bleiben ungeklärt.
Kein alter Lauf, Account oder Schlüssel wird übernommen. Arbeitsbereiche bleiben
inaktiv; fehlende Daten müssen über den separaten Backup-/Restorepfad behandelt
werden. Dieser Quellenstand ist noch keine reale VM-/Cluster-/Crashabnahme.
