"""Render and operate an explicitly owned Agent Sandbox topology (standard library).

No implicit context, credentials, runtime installation, PVC deletion or cluster
bootstrap. JSON output is a Kubernetes List accepted by kubectl.
"""
from __future__ import annotations

import argparse
import hashlib
import ipaddress
import json
from pathlib import Path
import re
import subprocess
import uuid

VERSION = "v1.0.7"
API = "agents.x-k8s.io/v1beta1"
OWNER = "cowork.local/deployment-id"
TENANT = "cowork.local/tenant-id"
NAME = "app.kubernetes.io/name"
PINNED_CRD = "https://raw.githubusercontent.com/kubernetes-sigs/agent-sandbox/v1.0.7/k8s/crds/agents.x-k8s.io_sandboxes.yaml"
CONTROLLER_IMAGE = "registry.k8s.io/agent-sandbox/agent-sandbox-controller@sha256:f254384c24605436ce35d7077427559e24bda8f5691cff5a54a4c2e030d81083"


def require(ok, message):
    if not ok:
        raise ValueError(message)


def dns(value):
    require(isinstance(value, str) and len(value) <= 63 and re.fullmatch(r"[a-z0-9](?:[-a-z0-9]*[a-z0-9])?", value), "Invalid DNS label")
    return value


def image(value):
    require(isinstance(value, str) and re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9._:/-]+@sha256:[a-f0-9]{64}", value), "Images require an immutable registry digest")
    return value


def validate(c):
    allowed = {"format", "deployment_id", "tenant_id", "context", "app_namespace", "controller_namespace", "tenant_namespace", "app_image", "runner_image", "controller_image", "init_image", "sandbox_image", "runtime_class", "runtime_handler", "storage_class", "data_storage_class", "workspace_access_mode", "data_access_mode", "api_cidrs", "dependency_cidrs", "database_host", "public_origin", "app_secret", "runner_secret", "runner_label", "namespace_uids", "quota", "windows", "ingress_namespace", "ingress_class", "ingress_tls_secret", "web_proxy", "egress_probe_address", "preview"}
    require(isinstance(c, dict) and not set(c) - allowed and c.get("format") == 1, "Unknown deployment configuration or format")
    for key in ("deployment_id", "tenant_id"):
        require(str(uuid.UUID(c[key])) == c[key], "Use canonical UUIDs")
    require(c["context"] and not any(x in c["context"] for x in "\r\n\0"), "Explicit kubectl context required")
    for key in ("app_namespace", "controller_namespace", "tenant_namespace", "runtime_class", "runtime_handler", "storage_class", "data_storage_class", "app_secret", "runner_secret", "ingress_namespace", "ingress_class", "ingress_tls_secret"):
        dns(c[key])
    require(len({c[k] for k in ("app_namespace", "controller_namespace", "tenant_namespace")}) == 3, "Control, controller and tenant namespaces must differ")
    for key in ("app_image", "runner_image", "controller_image", "init_image", "sandbox_image"):
        image(c[key])
    require(c["controller_image"] == CONTROLLER_IMAGE, "Select the verified upstream v1.0.7 controller digest")
    require(c["runtime_handler"].startswith("kata"), "Production workload handler must be an enrolled Kata handler")
    require(c.get("workspace_access_mode", "ReadWriteOncePod") in {"ReadWriteOncePod", "ReadWriteOnce"}, "Workspace access must be RWOP or explicit RWO")
    require(c.get("data_access_mode", "ReadWriteMany") == "ReadWriteMany", "HA app data requires qualified shared RWX storage")
    require(re.fullmatch(r"[a-f0-9]{32}", c["runner_label"]), "Runner label must match the first 32 hex characters of the adapter's canonical binding hash")
    require(c["public_origin"].startswith("https://") and re.fullmatch(r"https://[a-z0-9.-]+(?::[0-9]+)?", c["public_origin"]), "Explicit HTTPS origin required")
    require(re.fullmatch(r"[a-zA-Z0-9.-]+", c["database_host"]), "Invalid database host")
    for key in ("api_cidrs", "dependency_cidrs"):
        require(isinstance(c[key], list) and 0 < len(c[key]) <= 32, "Explicit API and dependency CIDRs required")
        for cidr in c[key]:
            require(ipaddress.ip_network(cidr, strict=True).prefixlen > 0, "Unbounded network range refused")
    address, port = c["egress_probe_address"].rsplit(":", 1)
    probe_ip = ipaddress.ip_address(address)
    require(probe_ip.version == 4 and not probe_ip.is_loopback and not probe_ip.is_unspecified and 1 <= int(port) <= 65535, "Explicit reachable IPv4 denial-canary socket required")
    for ns, identifier in c.get("namespace_uids", {}).items():
        require(ns in {c[k] for k in ("app_namespace", "controller_namespace", "tenant_namespace")} | {c[k]["namespace"] for k in ("windows", "web_proxy", "preview") if c.get(k)}, "Unknown namespace UID")
        uuid.UUID(identifier)
    quota = c.get("quota", {})
    require(not set(quota) - {"cpu", "memory_gib", "storage_gib", "sandboxes", "pvcs"}, "Unknown quota")
    for key, default in (("cpu", 16), ("memory_gib", 32), ("storage_gib", 200), ("sandboxes", 20), ("pvcs", 40)):
        require(type(quota.get(key, default)) is int and 0 < quota.get(key, default) <= 100000, "Positive bounded quotas required")
    if c.get("windows"):
        w = c["windows"]
        require(not set(w) - {"namespace", "namespace_uid", "vm_name", "boot_pvc", "cpu", "memory_gib", "agent_config_secret"}, "Unknown Windows configuration")
        for k in ("namespace", "vm_name", "boot_pvc", "agent_config_secret"):
            dns(w[k])
        require(w["namespace"] not in {c[k] for k in ("app_namespace", "controller_namespace", "tenant_namespace")}, "KubeVirt requires a separate operator-controlled namespace")
        require(type(w["cpu"]) is int and 1 <= w["cpu"] <= 32 and type(w["memory_gib"]) is int and 2 <= w["memory_gib"] <= 256, "Invalid Windows VM resource profile")
    if c.get("web_proxy"):
        w = c["web_proxy"]
        require(not set(w) - {"namespace", "image", "policy_id", "allowed_domains", "probe_url", "direct_probe_address"}, "Unknown egress proxy setting")
        dns(w["namespace"]); dns(w["policy_id"]); image(w["image"])
        require(w["namespace"] not in {c[k] for k in ("app_namespace", "controller_namespace", "tenant_namespace")}, "Proxy requires a separate namespace")
        domains = w["allowed_domains"]
        require(isinstance(domains, list) and 0 < len(domains) <= 64 and all(re.fullmatch(r"\.?[a-z0-9](?:[a-z0-9.-]{0,250}[a-z0-9])?", d) for d in domains), "Explicit bounded domain allowlist required")
        from urllib.parse import urlparse
        target = urlparse(w["probe_url"])
        require(target.scheme == "http" and target.hostname and not target.username and not target.password and any(target.hostname == d.lstrip(".") or d.startswith(".") and target.hostname.endswith(d) for d in domains), "HTTP probe must match the explicit domain allowlist")
        address, port = w["direct_probe_address"].rsplit(":", 1)
        require(ipaddress.ip_address(address).version == 4 and 1 <= int(port) <= 65535, "Explicit IPv4 direct-denial probe socket required")
    if c.get("preview"):
        p = c["preview"]
        require(not set(p) - {"namespace", "image", "domain", "tls_secret", "secret", "runner_id", "runner_generation", "runner_public_key", "allowed_ports"}, "Unknown preview setting")
        for key in ("namespace", "tls_secret", "secret"):
            dns(p[key])
        image(p["image"])
        require(re.fullmatch(r"[a-z0-9](?:[a-z0-9.-]{0,250}[a-z0-9])?", p["domain"]) and "." in p["domain"], "Explicit preview DNS base domain required")
        from urllib.parse import urlparse
        app_host = urlparse(c["public_origin"]).hostname
        require(app_host.split(".")[-2:] != p["domain"].split(".")[-2:] and not app_host.endswith("."+p["domain"]) and not p["domain"].endswith("."+app_host), "Untrusted previews require a different site from the Cowork application")
        require(isinstance(p["runner_id"], str) and re.fullmatch(r"[a-zA-Z0-9._:-]{1,128}", p["runner_id"]) and type(p["runner_generation"]) is int and p["runner_generation"] > 0, "Original runner binding required")
        require(isinstance(p["runner_public_key"], list) and len(p["runner_public_key"]) == 32 and all(type(n) is int and 0 <= n <= 255 for n in p["runner_public_key"]), "Original 32-byte runner public key required")
        require(isinstance(p["allowed_ports"], list) and 0 < len(p["allowed_ports"]) <= 8 and all(type(n) is int and 1024 < n <= 65535 and n not in {8080, 9090, 8443} for n in p["allowed_ports"]), "Explicit application preview ports required; sandboxd/control ports are forbidden")
    namespaces = [c[k] for k in ("app_namespace", "controller_namespace", "tenant_namespace")] + [c[k]["namespace"] for k in ("windows", "web_proxy", "preview") if c.get(k)]
    require(len(set(namespaces)) == len(namespaces), "Optional services require distinct namespaces")
    return c


def labels(c, component=None):
    return {OWNER: c["deployment_id"], TENANT: c["tenant_id"], **({NAME: component} if component else {})}


def obj(c, kind, name, namespace=None, api="v1", **fields):
    return {"apiVersion": api, "kind": kind, "metadata": {"name": name, "labels": labels(c), **({"namespace": namespace} if namespace else {})}, **fields}


def list_object(items):
    return {"apiVersion": "v1", "kind": "List", "items": items}


def role(c, name, ns, rules, cluster=False):
    return obj(c, "ClusterRole" if cluster else "Role", name, None if cluster else ns, "rbac.authorization.k8s.io/v1", rules=rules)


def bind(c, name, ns, account, account_ns, role_name, cluster=False):
    return obj(c, "ClusterRoleBinding" if cluster else "RoleBinding", name, None if cluster else ns, "rbac.authorization.k8s.io/v1", subjects=[{"kind": "ServiceAccount", "name": account, "namespace": account_ns}], roleRef={"apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole" if cluster else "Role", "name": role_name})


def rule(group, resources, verbs, names=None):
    return {"apiGroups": [group], "resources": resources, "verbs": verbs, **({"resourceNames": names} if names else {})}


def restricted():
    return {"allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True, "capabilities": {"drop": ["ALL"]}}


def peer(ns, selector):
    return {"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": ns}}, "podSelector": {"matchLabels": selector}}


def policy(c, name, ns, selector, ingress=None, egress=None):
    return obj(c, "NetworkPolicy", name, ns, "networking.k8s.io/v1", spec={"podSelector": {"matchLabels": selector}, "policyTypes": ["Ingress", "Egress"], "ingress": ingress or [], "egress": egress or []})


def cidr_egress(cidrs, ports):
    return {"to": [{"ipBlock": {"cidr": x}} for x in cidrs], "ports": [{"protocol": "TCP", "port": p} for p in ports]}


def dns_egress():
    return {"to": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}}, "podSelector": {"matchLabels": {"k8s-app": "kube-dns"}}}], "ports": [{"protocol": p, "port": 53} for p in ("UDP", "TCP")]}


def claim(c, name, ns, size, access, storage):
    # Deliberately no ownerReference: sandbox expiry, deletion, or claim cleanup
    # must not garbage-collect user data. Retention is a separate app decision.
    return obj(c, "PersistentVolumeClaim", name, ns, spec={"accessModes": [access], "storageClassName": storage, "resources": {"requests": {"storage": size}}})


def render(c):
    validate(c)
    app, controller, tenant = (c[k] for k in ("app_namespace", "controller_namespace", "tenant_namespace"))
    namespaces = []
    for ns in (app, controller, tenant):
        item = obj(c, "Namespace", ns)
        item["metadata"]["labels"].update({"pod-security.kubernetes.io/enforce": "baseline" if ns == app else "restricted", "pod-security.kubernetes.io/enforce-version": "latest"})
        namespaces.append(item)
    if c.get("windows"):
        windows_ns = obj(c, "Namespace", c["windows"]["namespace"])
        windows_ns["metadata"]["labels"]["cowork.local/purpose"] = "windows"
        namespaces.append(windows_ns)
    if c.get("web_proxy"):
        proxy_ns = obj(c, "Namespace", c["web_proxy"]["namespace"])
        proxy_ns["metadata"]["labels"]["pod-security.kubernetes.io/enforce"] = "restricted"
        namespaces.append(proxy_ns)
    if c.get("preview"):
        preview_ns = obj(c, "Namespace", c["preview"]["namespace"])
        preview_ns["metadata"]["labels"]["pod-security.kubernetes.io/enforce"] = "restricted"
        namespaces.append(preview_ns)
    q = {"cpu": 16, "memory_gib": 32, "storage_gib": 200, "sandboxes": 20, "pvcs": 40, **c.get("quota", {})}
    workload = [obj(c, "ServiceAccount", "cowork-workload", tenant, automountServiceAccountToken=False),
        obj(c, "ResourceQuota", "cowork-tenant", tenant, spec={"hard": {"requests.cpu": str(q["cpu"]), "limits.cpu": str(q["cpu"]), "requests.memory": f'{q["memory_gib"]}Gi', "limits.memory": f'{q["memory_gib"]}Gi', "requests.storage": f'{q["storage_gib"]}Gi', "persistentvolumeclaims": str(q["pvcs"]), "count/sandboxes.agents.x-k8s.io": str(q["sandboxes"]), "pods": str(q["sandboxes"] + 4)}}),
        obj(c, "LimitRange", "cowork-defaults", tenant, spec={"limits": [{"type": "Container", "default": {"cpu": "1", "memory": "2Gi", "ephemeral-storage": "2Gi"}, "defaultRequest": {"cpu": "100m", "memory": "128Mi", "ephemeral-storage": "128Mi"}}]}),
        policy(c, "default-deny", tenant, {})]
    # Runtime policy is cluster-admin-installed; application accounts cannot
    # choose ordinary runc, another image, or an ambient ServiceAccount token.
    validations = [
        {"expression": f"object.spec.runtimeClassName == '{c['runtime_class']}'", "message": "An enrolled Kata RuntimeClass is required"},
        {"expression": "has(object.spec.automountServiceAccountToken) && object.spec.automountServiceAccountToken == false", "message": "Workloads cannot receive Kubernetes credentials"},
        {"expression": "object.spec.serviceAccountName == 'cowork-workload'", "message": "Dedicated tokenless workload account required"},
        {"expression": "size(object.spec.containers) == 1 && (!has(object.spec.initContainers) || size(object.spec.initContainers) == 0) && (!has(object.spec.ephemeralContainers) || size(object.spec.ephemeralContainers) == 0)", "message": "Sidecars and injected debuggers are outside this qualified profile"},
        {"expression": "!has(object.spec.hostNetwork) || !object.spec.hostNetwork", "message": "Host network refused"},
        {"expression": "!has(object.spec.volumes) || object.spec.volumes.all(v, !has(v.hostPath) && !has(v.secret) && !has(v.projected))", "message": "Host paths and credentials refused"},
        {"expression": f"object.spec.containers.all(c, c.image == '{c['sandbox_image']}')", "message": "Only the qualified immutable workload image is accepted"},
    ]
    admission_name = "cowork-workloads-" + c["deployment_id"][:8]
    cluster = [obj(c, "ValidatingAdmissionPolicy", admission_name, api="admissionregistration.k8s.io/v1", spec={"failurePolicy": "Fail", "matchConstraints": {"resourceRules": [{"apiGroups": [""], "apiVersions": ["v1"], "operations": ["CREATE", "UPDATE"], "resources": ["pods"]}]}, "validations": validations}),
        obj(c, "ValidatingAdmissionPolicyBinding", admission_name, api="admissionregistration.k8s.io/v1", spec={"policyName": admission_name, "validationActions": ["Deny"], "matchResources": {"namespaceSelector": {"matchLabels": {OWNER: c["deployment_id"], "cowork.local/purpose": "workload"}}}})]
    namespaces[2]["metadata"]["labels"]["cowork.local/purpose"] = "workload"
    # Upstream controller caches cluster-wide. Read-only cache permissions are
    # cluster-scoped; writes are granted only in this enrolled tenant namespace.
    cache_role = "cowork-sandbox-cache-" + c["deployment_id"][:8]
    cluster += [role(c, cache_role, None, [rule("agents.x-k8s.io", ["sandboxes"], ["get", "list", "watch"]), rule("", ["pods", "services", "persistentvolumeclaims"], ["get", "list", "watch"])], True),
        bind(c, cache_role, None, "cowork-sandbox-controller", controller, cache_role, True)]
    observer_role = "cowork-runner-observer-" + c["deployment_id"][:8]
    cluster += [role(c, observer_role, None, [rule("", ["namespaces"], ["get"], [tenant]), rule("node.k8s.io", ["runtimeclasses"], ["get"], [c["runtime_class"]])], True), bind(c, observer_role, None, "cowork-process-runner", app, observer_role, True)]
    workload += [role(c, "cowork-runner", tenant, [rule("agents.x-k8s.io", ["sandboxes"], ["get", "list", "create", "patch", "delete"]), rule("", ["pods"], ["get", "list", "patch"]), rule("", ["services"], ["get"]), rule("discovery.k8s.io", ["endpointslices"], ["list"]), rule("", ["persistentvolumeclaims"], ["get", "create"]), rule("networking.k8s.io", ["networkpolicies"], ["get", "list", "create", "delete"])]), bind(c, "cowork-runner", tenant, "cowork-process-runner", app, "cowork-runner"),
        role(c, "cowork-controller", tenant, [rule("agents.x-k8s.io", ["sandboxes", "sandboxes/status", "sandboxes/finalizers"], ["get", "list", "watch", "update", "patch"]), rule("", ["pods", "services"], ["get", "list", "watch", "create", "update", "patch", "delete"]), rule("", ["persistentvolumeclaims"], ["get", "list", "watch"]), rule("events.k8s.io", ["events"], ["create", "patch", "update"]), rule("", ["events"], ["create", "patch", "update"])]), bind(c, "cowork-controller", tenant, "cowork-sandbox-controller", controller, "cowork-controller")]
    controls = [obj(c, "ServiceAccount", "cowork-sandbox-controller", controller),
        role(c, "cowork-controller-leader", controller, [rule("coordination.k8s.io", ["leases"], ["get", "list", "watch", "create", "update", "patch"])]), bind(c, "cowork-controller-leader", controller, "cowork-sandbox-controller", controller, "cowork-controller-leader")]
    ctrl_labels = {NAME: "cowork-sandbox-controller"}
    ctrl_container = {"name": "controller", "image": c["controller_image"], "args": ["--leader-elect=true", "--leader-election-namespace=" + controller, "--extensions=false", "--sandbox-concurrent-workers=20", "--metrics-bind-address=:8080", "--health-probe-bind-address=:8081"], "securityContext": restricted(), "ports": [{"name": "health", "containerPort": 8081}], "readinessProbe": {"httpGet": {"path": "/readyz", "port": "health"}}, "livenessProbe": {"httpGet": {"path": "/healthz", "port": "health"}}, "resources": {"requests": {"cpu": "100m", "memory": "128Mi"}, "limits": {"cpu": "1", "memory": "512Mi"}}}
    controls.append(obj(c, "Deployment", "cowork-sandbox-controller", controller, "apps/v1", spec={"replicas": 2, "selector": {"matchLabels": ctrl_labels}, "template": {"metadata": {"labels": {**labels(c), **ctrl_labels}, "annotations": {"cowork.local/upstream-version": VERSION}}, "spec": {"serviceAccountName": "cowork-sandbox-controller", "securityContext": {"runAsNonRoot": True, "runAsUser": 65532, "seccompProfile": {"type": "RuntimeDefault"}}, "containers": [ctrl_container], "topologySpreadConstraints": [{"maxSkew": 1, "topologyKey": "kubernetes.io/hostname", "whenUnsatisfiable": "DoNotSchedule", "labelSelector": {"matchLabels": ctrl_labels}}]}}}))
    controls += [obj(c, "PodDisruptionBudget", "cowork-controller", controller, "policy/v1", spec={"minAvailable": 1, "selector": {"matchLabels": ctrl_labels}}), policy(c, "default-deny", controller, {}), policy(c, "controller-api", controller, ctrl_labels, egress=[dns_egress(), cidr_egress(c["api_cidrs"], [443, 6443])])]
    application = [obj(c, "ServiceAccount", "cowork-app", app, automountServiceAccountToken=False), obj(c, "ServiceAccount", "cowork-process-runner", app, automountServiceAccountToken=False), claim(c, "cowork-data", app, "20Gi", "ReadWriteMany", c["data_storage_class"]), claim(c, "cowork-runner-journal", app, "10Gi", "ReadWriteOncePod", c["storage_class"]), policy(c, "default-deny", app, {})]
    # Existing secret material is copied to regular private files; native key
    # custody intentionally rejects Kubernetes projection symlinks.
    for component in ("api", "worker", "process-runner"):
        runner = component == "process-runner"
        appname = "cowork-" + component
        # Broker grants and locks bind the actual kernel filesystem identity.
        # RWX access on another node does not promise an identical st_dev.
        broker_group = {"cowork.local/broker-group": c["deployment_id"]}
        podlabels = {**labels(c, appname), **broker_group, **({"cowork.local/runner": c["runner_label"]} if runner else {})}
        secret = c["runner_secret"] if runner else c["app_secret"]
        script = "umask 077; mkdir -p /private/keys; cp -L /projected/* /private/keys/; chmod 700 /private/keys; chmod 600 /private/keys/*"
        if runner:
            script += "; mkdir -p /state/runner /broker-source/file-broker; chmod 700 /state/runner /broker-source/file-broker"
        init = {"name": "private-custody", "image": c["init_image"], "command": ["/bin/sh", "-ec", script], "securityContext": restricted(), "resources": {"requests": {"cpu": "10m", "memory": "16Mi"}, "limits": {"cpu": "100m", "memory": "64Mi"}}, "volumeMounts": [{"name": "projected", "mountPath": "/projected", "readOnly": True}, {"name": "private", "mountPath": "/private"}]}
        if runner:
            init["volumeMounts"].append({"name": "data", "mountPath": "/state"})
            init["volumeMounts"].append({"name": "broker", "mountPath": "/broker-source"})
        env = [{"name": "COWORK_DATA_DIR", "value": "/var/lib/cowork"}, {"name": "COWORK_BIND_ADDRESS", "value": "0.0.0.0"}, {"name": "COWORK_PUBLIC_ORIGIN", "value": c["public_origin"]}, {"name": "COWORK_ENABLE_TENANT_IDENTITY", "value": "1"}, {"name": "COWORK_DATABASE_HOST", "value": c["database_host"]}, {"name": "COWORK_DATABASE_PASSWORD_FILE", "value": "/run/private/keys/postgres_password"}, {"name": "COWORK_DATABASE_SSLMODE", "value": "verify-full"}, {"name": "COWORK_DATABASE_SSLROOTCERT", "value": "/run/private/keys/postgres_ca.pem"}, {"name": "COWORK_PROCESS_RUNNER_CONFIG", "value": "/run/private/keys/runner-client.json"}]
        if c.get("preview"):
            env.append({"name": "COWORK_SANDBOX_PREVIEW_BASE_DOMAIN", "value": c["preview"]["domain"]})
        container = {"name": component, "image": c["runner_image"] if runner else c["app_image"], "securityContext": restricted(), "resources": {"requests": {"cpu": "100m", "memory": "256Mi"}, "limits": {"cpu": "1", "memory": "1Gi"}}, "volumeMounts": [{"name": "data", "mountPath": "/var/lib/cowork-runner" if runner else "/var/lib/cowork"}, {"name": "private", "mountPath": "/run/private", "readOnly": True}, {"name": "tmp", "mountPath": "/tmp"}]}
        volumes = [{"name": "data", "persistentVolumeClaim": {"claimName": "cowork-runner-journal" if runner else "cowork-data"}}, {"name": "projected", "secret": {"secretName": secret}}, {"name": "private", "emptyDir": {"medium": "Memory", "sizeLimit": "8Mi"}}, {"name": "tmp", "emptyDir": {"sizeLimit": "256Mi"}}]
        if runner:
            container.update(command=["/usr/local/bin/cowork-process-runner"], args=["serve", "/run/private/keys/config.json"], ports=[{"containerPort": 8443, "name": "https"}])
            container["volumeMounts"].append({"name": "kube-api", "mountPath": "/var/run/cowork-kubernetes", "readOnly": True})
            container["volumeMounts"].append({"name": "broker", "mountPath": "/var/lib/cowork/file-broker", "subPath": "file-broker"})
            volumes.append({"name": "broker", "persistentVolumeClaim": {"claimName": "cowork-data"}})
            volumes.append({"name": "kube-api", "projected": {"defaultMode": 0o444, "sources": [{"serviceAccountToken": {"path": "token", "expirationSeconds": 3600}}, {"configMap": {"name": "kube-root-ca.crt", "items": [{"key": "ca.crt", "path": "ca.crt"}]}}]}})
        else:
            container.update(args=["--worker-only" if component == "worker" else "--no-worker"], env=env)
            if component == "api":
                container.update(ports=[{"containerPort": 43821, "name": "http"}], readinessProbe={"httpGet": {"path": "/health", "port": "http"}})
        custody_script = "test ! -L /state; chown 1000:1000 /state; chmod 700 /state"
        custody_mounts = [{"name": "data", "mountPath": "/state"}]
        if runner:
            custody_script += "; test ! -L /broker-source; chown 1000:1000 /broker-source; chmod 700 /broker-source; mkdir -p /broker-source/file-broker; test ! -L /broker-source/file-broker; chown 1000:1000 /broker-source/file-broker; chmod 700 /broker-source/file-broker"
            custody_mounts.append({"name": "broker", "mountPath": "/broker-source"})
        custody = {"name": "volume-custody", "image": c["init_image"], "command": ["/bin/sh", "-ec", custody_script], "securityContext": {"runAsNonRoot": False, "runAsUser": 0, "allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True, "capabilities": {"drop": ["ALL"], "add": ["CHOWN", "DAC_OVERRIDE", "FOWNER"]}}, "resources": {"requests": {"cpu": "10m", "memory": "16Mi"}, "limits": {"cpu": "100m", "memory": "64Mi"}}, "volumeMounts": custody_mounts}
        # fsGroup can recursively widen retained 0600 credential files to 0660
        # on another replica's mount. Root-only custody prepares volume roots;
        # all actual writers then use the original same UID without fsGroup.
        pod = {"serviceAccountName": "cowork-process-runner" if runner else "cowork-app", "automountServiceAccountToken": False, "terminationGracePeriodSeconds": 120, "securityContext": {"runAsNonRoot": True, "runAsUser": 1000, "runAsGroup": 1000, "seccompProfile": {"type": "RuntimeDefault"}}, "initContainers": [custody, init], "containers": [container], "volumes": volumes}
        pod["affinity"] = {"podAffinity": {"requiredDuringSchedulingIgnoredDuringExecution": [{"labelSelector": {"matchLabels": broker_group}, "topologyKey": "kubernetes.io/hostname"}]}}
        spec = {"replicas": 1 if runner else 2, "selector": {"matchLabels": {NAME: appname}}, "template": {"metadata": {"labels": podlabels}, "spec": pod}}
        if runner:
            spec["serviceName"] = appname
        application.append(obj(c, "StatefulSet" if runner else "Deployment", appname, app, "apps/v1", spec=spec))
        if not runner:
            application.append(obj(c, "PodDisruptionBudget", appname, app, "policy/v1", spec={"minAvailable": 1, "selector": {"matchLabels": {NAME: appname}}}))
    application += [obj(c, "Service", "cowork-api", app, spec={"selector": {NAME: "cowork-api"}, "ports": [{"port": 43821, "targetPort": "http"}]}), obj(c, "Service", "cowork-process-runner", app, spec={"selector": {NAME: "cowork-process-runner"}, "ports": [{"port": 443, "targetPort": "https"}]})]
    for component in ("api", "worker"):
        application.append(policy(c, component + "-dependencies", app, {NAME: "cowork-" + component}, egress=[dns_egress(), cidr_egress(c["dependency_cidrs"], [443, 5432, 9000]), {"to": [{"podSelector": {"matchLabels": {NAME: "cowork-process-runner"}}}], "ports": [{"protocol": "TCP", "port": 8443}]}]))
    application += [policy(c, "runner-worker", app, {NAME: "cowork-process-runner"}, ingress=[{"from": [{"podSelector": {"matchLabels": {NAME: n}}} for n in ("cowork-api", "cowork-worker")], "ports": [{"protocol": "TCP", "port": 8443}]}], egress=[dns_egress(), cidr_egress(c["api_cidrs"], [443, 6443]), {"to": [peer(tenant, {NAME: "cowork-sandbox", "cowork.local/runner": c["runner_label"]})], "ports": [{"protocol": "TCP", "port": p} for p in ([8080, 9090] + c.get("preview", {}).get("allowed_ports", []))]}]), policy(c, "ingress-api", app, {NAME: "cowork-api"}, ingress=[{"from": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": c["ingress_namespace"]}}}], "ports": [{"protocol": "TCP", "port": 43821}]}]), obj(c, "Ingress", "cowork-api", app, "networking.k8s.io/v1", spec={"ingressClassName": c["ingress_class"], "tls": [{"hosts": [c["public_origin"].split("//", 1)[1].split(":", 1)[0]], "secretName": c["ingress_tls_secret"]}], "rules": [{"host": c["public_origin"].split("//", 1)[1].split(":", 1)[0], "http": {"paths": [{"path": "/", "pathType": "Prefix", "backend": {"service": {"name": "cowork-api", "port": {"number": 43821}}}}]}}]})]
    # Ingress deliberately needs the operator's ingress-controller identity;
    # there is no public NodePort or wildcard inbound rule in this artifact.
    profiles = {"upstream_version": VERSION, "live_qualified": False, "profiles": {name: {"runtime_class": c["runtime_class"], "runtime_handler": c["runtime_handler"], "image": c["sandbox_image"], "cpu_millis": cpu, "memory_mib": memory, "pids": pids, "storage_gib": storage, "network": "denied"} for name, cpu, memory, pids, storage in [("small", 1000, 2048, 128, 10), ("browser", 2000, 4096, 512, 20), ("large", 4000, 8192, 512, 40)]}}
    result = {"namespaces": list_object(namespaces), "cluster-policy": list_object(cluster), "controller": list_object(controls), "tenant": list_object(workload), "application": list_object(application), "profiles": profiles}
    settings = {"api_url": "https://kubernetes.default.svc", "namespace": tenant, "namespace_uid": c.get("namespace_uids", {}).get(tenant, "ENROLL_NAMESPACE_UID_FIRST"), "token_file": "/var/run/cowork-kubernetes/token", "ca_file": "/var/run/cowork-kubernetes/ca.crt", "runtime_class": c["runtime_class"], "runtime_handler": c["runtime_handler"], "storage_class": c["storage_class"], "pvc_access_mode": c.get("workspace_access_mode", "ReadWriteOncePod"), "worker_namespace": app, "worker_label_value": c["runner_label"], "approved_images": [c["sandbox_image"]], "cluster_domain": "cluster.local", "web_proxy_policies": {}, "egress_probe_address": c["egress_probe_address"], "readiness_timeout_seconds": 180, "stop_timeout_seconds": 120}
    if c.get("web_proxy"):
        w = c["web_proxy"]; pns = w["namespace"]
        proxy_labels = {NAME: "cowork-egress-proxy", "cowork.local/proxy-policy": w["policy_id"]}
        squid = "\n".join(["http_port 3128", "pid_filename /tmp/squid.pid", "cache deny all", "access_log stdio:/dev/stdout", "cache_log /dev/stderr", "cache_store_log none", "acl CONNECT method CONNECT", "acl safe_ports port 80 443", "acl tls_ports port 443", "acl private_dst dst 0.0.0.0/8 10.0.0.0/8 100.64.0.0/10 127.0.0.0/8 169.254.0.0/16 172.16.0.0/12 192.168.0.0/16 224.0.0.0/4 240.0.0.0/4 ::/0", "acl allowed_domains dstdomain " + " ".join(w["allowed_domains"]), "http_access deny !safe_ports", "http_access deny CONNECT !tls_ports", "http_access deny private_dst", "http_access allow allowed_domains", "http_access deny all", "forwarded_for delete", "via off", "shutdown_lifetime 1 seconds", ""])
        proxy_container = {"name": "proxy", "image": w["image"], "securityContext": restricted(), "ports": [{"containerPort": 3128, "name": "proxy"}], "resources": {"requests": {"cpu": "100m", "memory": "128Mi"}, "limits": {"cpu": "1", "memory": "512Mi"}}, "volumeMounts": [{"name": "config", "mountPath": "/etc/squid/squid.conf", "subPath": "squid.conf", "readOnly": True}, {"name": "tmp", "mountPath": "/tmp"}], "readinessProbe": {"tcpSocket": {"port": "proxy"}}}
        result["egress-proxy"] = list_object([obj(c, "ConfigMap", "cowork-egress-proxy", pns, data={"squid.conf": squid}), obj(c, "Deployment", "cowork-egress-proxy", pns, "apps/v1", spec={"replicas": 2, "selector": {"matchLabels": proxy_labels}, "template": {"metadata": {"labels": {**labels(c), **proxy_labels}}, "spec": {"automountServiceAccountToken": False, "securityContext": {"runAsNonRoot": True, "runAsUser": 1000, "runAsGroup": 1000, "seccompProfile": {"type": "RuntimeDefault"}}, "containers": [proxy_container], "volumes": [{"name": "config", "configMap": {"name": "cowork-egress-proxy"}}, {"name": "tmp", "emptyDir": {"sizeLimit": "128Mi"}}]}}}), obj(c, "Service", "cowork-egress-proxy", pns, spec={"selector": proxy_labels, "ports": [{"port": 3128, "targetPort": "proxy"}]}), policy(c, "default-deny", pns, {}), policy(c, "sandbox-proxy", pns, proxy_labels, ingress=[{"from": [peer(tenant, {NAME: "cowork-sandbox", "cowork.local/runner": c["runner_label"]})], "ports": [{"protocol": "TCP", "port": 3128}]}], egress=[dns_egress(), {"to": [{"ipBlock": {"cidr": "0.0.0.0/0", "except": ["0.0.0.0/8", "10.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8", "169.254.0.0/16", "172.16.0.0/12", "192.168.0.0/16", "224.0.0.0/4", "240.0.0.0/4"]}}], "ports": [{"protocol": "TCP", "port": p} for p in (80, 443)]}])])
        settings["web_proxy_policies"][w["policy_id"]] = {"url": f"http://cowork-egress-proxy.{pns}.svc.cluster.local:3128", "namespace": pns, "pod_labels": proxy_labels, "port": 3128, "probe_url": w["probe_url"], "direct_probe_address": w["direct_probe_address"]}
    if c.get("preview"):
        p = c["preview"]; pns = p["namespace"]
        plabels = {NAME: "cowork-sandbox-preview", "cowork.local/runner": c["runner_label"]}
        preview_settings = {"format": 1, "listen": "0.0.0.0:8080", "preview_domain": p["domain"], "app_origin": c["public_origin"], "namespace": tenant, "service_suffix": f".{tenant}.svc.cluster.local", "allowed_ports": p["allowed_ports"], "runner_id": p["runner_id"], "runner_generation": p["runner_generation"], "runner_public_key": p["runner_public_key"], "runner_url": f"https://cowork-process-runner.{app}.svc.cluster.local/", "runner_bearer_file": "/run/private/keys/preview-validator-bearer", "runner_ca_file": "/run/private/keys/runner-ca.pem", "max_ticket_ms": 60000}
        init = {"name": "private-custody", "image": c["init_image"], "command": ["/bin/sh", "-ec", "umask 077; mkdir -p /private/keys; cp -L /projected/* /private/keys/; cp -L /settings/config.json /private/keys/preview-config.json; chmod 700 /private/keys; chmod 600 /private/keys/*"], "securityContext": restricted(), "resources": {"requests": {"cpu": "10m", "memory": "16Mi"}, "limits": {"cpu": "100m", "memory": "64Mi"}}, "volumeMounts": [{"name": "secret", "mountPath": "/projected", "readOnly": True}, {"name": "private", "mountPath": "/private"}, {"name": "config", "mountPath": "/settings", "readOnly": True}]}
        container = {"name": "gateway", "image": p["image"], "command": ["/usr/local/bin/cowork-sandbox-preview"], "args": ["--config", "/run/private/keys/preview-config.json"], "securityContext": restricted(), "ports": [{"containerPort": 8080, "name": "http"}], "readinessProbe": {"httpGet": {"path": "/healthz", "port": "http"}}, "resources": {"requests": {"cpu": "100m", "memory": "128Mi"}, "limits": {"cpu": "1", "memory": "512Mi"}}, "volumeMounts": [{"name": "private", "mountPath": "/run/private", "readOnly": True}, {"name": "tmp", "mountPath": "/tmp"}]}
        deployment = obj(c, "Deployment", "cowork-sandbox-preview", pns, "apps/v1", spec={"replicas": 2, "selector": {"matchLabels": plabels}, "template": {"metadata": {"labels": {**labels(c), **plabels}}, "spec": {"automountServiceAccountToken": False, "securityContext": {"runAsNonRoot": True, "runAsUser": 1000, "runAsGroup": 1000, "fsGroup": 1000, "seccompProfile": {"type": "RuntimeDefault"}}, "initContainers": [init], "containers": [container], "volumes": [{"name": "secret", "secret": {"secretName": p["secret"]}}, {"name": "private", "emptyDir": {"medium": "Memory", "sizeLimit": "8Mi"}}, {"name": "config", "configMap": {"name": "cowork-sandbox-preview"}}, {"name": "tmp", "emptyDir": {"sizeLimit": "128Mi"}}]}}})
        result["preview"] = list_object([obj(c, "ConfigMap", "cowork-sandbox-preview", pns, data={"config.json": json.dumps(preview_settings)}), deployment, obj(c, "Service", "cowork-sandbox-preview", pns, spec={"selector": plabels, "ports": [{"port": 8080, "targetPort": "http"}]}), obj(c, "PodDisruptionBudget", "cowork-sandbox-preview", pns, "policy/v1", spec={"minAvailable": 1, "selector": {"matchLabels": plabels}}), policy(c, "default-deny", pns, {}), policy(c, "preview-private-routes", pns, plabels, ingress=[{"from": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": c["ingress_namespace"]}}}], "ports": [{"protocol": "TCP", "port": 8080}]}], egress=[dns_egress(), {"to": [peer(app, {NAME: "cowork-process-runner"})], "ports": [{"protocol": "TCP", "port": 8443}]}, {"to": [peer(tenant, {NAME: "cowork-sandbox", "cowork.local/runner": c["runner_label"]})], "ports": [{"protocol": "TCP", "port": port} for port in p["allowed_ports"]]}]), obj(c, "Ingress", "cowork-sandbox-preview", pns, "networking.k8s.io/v1", spec={"ingressClassName": c["ingress_class"], "tls": [{"hosts": ["*." + p["domain"]], "secretName": p["tls_secret"]}], "rules": [{"host": "*." + p["domain"], "http": {"paths": [{"path": "/", "pathType": "Prefix", "backend": {"service": {"name": "cowork-sandbox-preview", "port": {"number": 8080}}}}]}}]})])
        application.append(policy(c, "preview-runner", app, {NAME: "cowork-process-runner"}, ingress=[{"from": [peer(pns, plabels)], "ports": [{"protocol": "TCP", "port": 8443}]}]))
        settings["preview_gateway"] = {"namespace": pns, "pod_labels": plabels, "ports": p["allowed_ports"]}
        settings["preview_base_domain"] = p["domain"]
        settings["preview_allowed_ports"] = p["allowed_ports"]
        settings["preview_validator_token_file"] = "/run/private/keys/preview-validator-bearer"
    result["adapter-settings"] = {"agent_sandbox": settings}
    result["runner-paths"] = {"data_directory": "/var/lib/cowork-runner/runner", "workspace_state_directory": "/var/lib/cowork/file-broker", "probe_executable": "/usr/local/bin/cowork-runner-probe", "docker_executable": None}
    if c.get("windows"):
        w = c["windows"]
        vm = obj(c, "VirtualMachine", w["vm_name"], w["namespace"], "kubevirt.io/v1", spec={"runStrategy": "Halted", "template": {"metadata": {"labels": labels(c, "cowork-windows")}, "spec": {"domain": {"cpu": {"cores": w["cpu"]}, "resources": {"requests": {"memory": f'{w["memory_gib"]}Gi'}, "limits": {"memory": f'{w["memory_gib"]}Gi', "cpu": str(w["cpu"])}}, "devices": {"interfaces": [{"name": "default", "masquerade": {}}], "disks": [{"name": "boot", "disk": {"bus": "sata"}}, {"name": "agent-config", "cdrom": {"bus": "sata"}}]}}, "volumes": [{"name": "boot", "persistentVolumeClaim": {"claimName": w["boot_pvc"]}}, {"name": "agent-config", "secret": {"secretName": w["agent_config_secret"]}}], "networks": [{"name": "default", "pod": {}}]}}})
        result["windows"] = list_object([vm])
    return result


class Cluster:
    def __init__(self, config, run=subprocess.run):
        self.config, self.run = validate(config), run

    def call(self, *args, data=None):
        result = self.run(["kubectl", "--context", self.config["context"], *args], input=None if data is None else json.dumps(data), text=True, capture_output=True, timeout=180, check=False)
        require(result.returncode == 0, "kubectl operation failed; inspect the operator terminal (credential output is withheld)")
        raw = result.stdout.strip()
        return json.loads(raw) if raw.startswith(("{", "[")) else raw or None

    def verify(self):
        names = [self.config[k] for k in ("app_namespace", "controller_namespace", "tenant_namespace")]
        if self.config.get("windows"):
            names.append(self.config["windows"]["namespace"])
        for key in ("web_proxy", "preview"):
            if self.config.get(key):
                names.append(self.config[key]["namespace"])
        for ns in names:
            expected = self.config.get("namespace_uids", {}).get(ns)
            require(expected, "Initialize and pin all namespace UIDs before cluster operations")
            observed = self.call("get", "namespace", ns, "-o", "json")
            require(observed["metadata"]["uid"] == expected and observed["metadata"].get("labels", {}).get(OWNER) == self.config["deployment_id"], "Namespace ownership changed")

    def apply(self, target):
        require(target in {"controller", "tenant", "application", "windows", "egress-proxy", "preview"}, "Cluster policy/CRD bootstrap requires a separate cluster administrator")
        self.verify()
        manifest = render(self.config)[target]
        # Server-side apply is allowed only for absent or already-owned objects.
        # No namespace adoption, force-conflicts, replace, prune, or PVC removal.
        for item in manifest["items"]:
            m = item["metadata"]
            observed = self.call("get", item["kind"], m["name"], "--namespace", m["namespace"], "--ignore-not-found", "-o", "json")
            require(observed is None or observed["metadata"].get("labels", {}).get(OWNER) == self.config["deployment_id"], "Existing resource belongs to a different installation")
        return self.call("apply", "--server-side", "--field-manager=cowork-agent-sandbox", "-f", "-", data=manifest)

    def stop(self):
        self.verify()
        # Stop admission first. Suspension is not a physical stop receipt and
        # never releases app quota or deletes workspace claims.
        ns = self.config["app_namespace"]
        for component in ("cowork-worker", "cowork-api"):
            found = self.call("get", "deployment", component, "-n", ns, "-o", "json")
            require(found["metadata"].get("labels", {}).get(OWNER) == self.config["deployment_id"], "Writer ownership changed")
            self.call("scale", "deployment/" + component, "-n", ns, "--replicas=0")
        found = self.call("get", "sandboxes", "-n", self.config["tenant_namespace"], "-l", "cowork.local/runner=" + self.config["runner_label"], "-o", "json")
        for item in found["items"]:
            self.call("patch", "sandbox", item["metadata"]["name"], "-n", self.config["tenant_namespace"], "--type=json", "-p", json.dumps([{"op": "test", "path": "/metadata/uid", "value": item["metadata"]["uid"]}, {"op": "add", "path": "/spec/operatingMode", "value": "Suspended"}]))
        return {"admission_stopped": True, "physical_stop_confirmed": False, "pvc_deleted": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["render", "apply", "inspect", "stop", "bootstrap-info", "enroll-namespaces"])
    parser.add_argument("--config", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--target", choices=["controller", "tenant", "application", "windows", "egress-proxy", "preview"])
    args = parser.parse_args()
    if args.command == "bootstrap-info":
        print(json.dumps({"version": VERSION, "crd_url": PINNED_CRD, "cluster_admin_required": True, "automatic_install": False}, indent=2))
        return
    require(args.config is not None, "Explicit configuration file required")
    c = validate(json.loads(args.config.read_text(encoding="utf-8")))
    if args.command == "render":
        require(args.output and args.output.is_absolute() and not args.output.exists(), "Render into a new absolute output directory")
        args.output.mkdir(parents=True, mode=0o700)
        for name, value in render(c).items():
            (args.output / (name + ".json")).write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({"version": VERSION, "rendered": str(args.output), "source_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), "live_qualified": False}))
    elif args.command == "enroll-namespaces":
        cluster = Cluster(c)
        identities = {}
        for item in render(c)["namespaces"]["items"]:
            ns = item["metadata"]["name"]
            observed = cluster.call("get", "namespace", ns, "-o", "json")
            require(observed["metadata"].get("labels", {}).get(OWNER) == c["deployment_id"], "Cannot enroll a foreign namespace")
            identifier = observed["metadata"]["uid"]
            require(c.get("namespace_uids", {}).get(ns, identifier) == identifier, "Previously enrolled namespace identity changed")
            identities[ns] = identifier
        c["namespace_uids"] = identities
        require(args.output and args.output.is_absolute() and not args.output.exists(), "Write enrolled settings to a new absolute config path")
        with args.output.open("x", encoding="utf-8") as out:
            json.dump(c, out, indent=2)
            out.write("\n")
    elif args.command == "apply":
        require(args.target is not None, "Explicit apply target required")
        Cluster(c).apply(args.target)
    elif args.command == "stop":
        print(json.dumps(Cluster(c).stop()))
    else:
        cluster = Cluster(c)
        cluster.verify()
        print(json.dumps(cluster.call("get", "sandboxes,pvc,pods", "-n", c["tenant_namespace"], "-o", "json"), indent=2))


if __name__ == "__main__":
    main()
