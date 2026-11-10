#!/usr/bin/env python3
"""Operator-owned fleet provisioner. No user shell commands or manifest fragments.

All infrastructure configuration is loaded from a local, operator-owned JSON file.
The manager supplies only typed UUIDs, actions and resource limits. Receipts are
written before reporting completion, so losing the response cannot replay work.
"""
from __future__ import annotations

import argparse
import base64
import json
import hashlib
import hmac
import ipaddress
import os
from pathlib import Path
import re
import secrets
import ssl
import subprocess
import time
from urllib.parse import unquote, urlsplit
from urllib.request import Request, build_opener, HTTPRedirectHandler, HTTPSHandler, ProxyHandler
import uuid
from instance_backups import InstanceBackups, BackupError, pg_environment
from operation_journal import OperationJournal, JournalError, operator_lock, operator_pass_fds
from fleet_images import retain_fleet_images
from instance_updates import InstanceUpdates
from instance_rollbacks import InstanceRollbacks
from instance_deletion import InstanceDeletion, DeletionError
from release_catalog import ReleaseCatalog, ReleaseError, image_set_id
import organisation_mfa
import execution_generation
import execution_runtime
import execution_recovery
import instance_enrollment
import provisioning_release
import resource_budget
import executor_host
import backend_profile
import environment_lifecycle
import environment_execution
import vm_snapshots
import instance_disk_quotas
import guest_endpoints
import instance_gateway
import host_capacity_attestation
import remote_lifecycle_qualification
from execution_generation import ExecutionGenerationError


class ProvisioningError(Exception):
    def __init__(self, code: str, instance_state: str | None = None, result: dict | None = None):
        super().__init__(code)
        self.instance_state = instance_state
        self.result = result or {}


def identifier(value: str) -> str:
    parsed = uuid.UUID(value)
    if str(parsed) != value:
        raise ProvisioningError("noncanonical_identifier")
    return parsed.hex


def immutable_image(value: str) -> str:
    if not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9._:/-]*@sha256:[0-9a-f]{64}", value):
        raise ProvisioningError("image_must_be_digest_pinned")
    return value


def resource_names(instance: dict, directory: Path) -> dict:
    compact = identifier(instance["id"])
    marker = directory / "generation.json"
    generation = None
    if marker.exists():
        if marker.is_symlink():
            raise ProvisioningError("unsafe_generation")
        generation = json.loads(marker.read_text())["generation"]
        identifier(generation)
    storage_id = hashlib.sha256((instance["id"] + ":" + generation).encode()).hexdigest()[:32] if generation else compact
    return {"database": "ci_" + storage_id, "bucket": "cowork-" + storage_id, "generation": generation,
            "volume_prefix": "cowork-" + compact + ("-g-" + identifier(generation) if generation else "") + "-run-"}


def resource_limits(instance: dict) -> dict:
    limits = instance["policy"]["limits"]
    for key, minimum, maximum in [("cpu_millis", 100, 1_024_000), ("memory_mib", 128, 4_194_304), ("storage_gib", 1, 1_048_576), ("concurrent_runs", 1, 1000)]:
        if type(limits.get(key)) is not int or not minimum <= limits[key] <= maximum:
            raise ProvisioningError("invalid_resource_limits")
    return limits


def allocate_subnets(pool: str, sizes: dict, occupied: list) -> dict:
    network = ipaddress.ip_network(pool)
    if network.version != 4 or not any(network.subnet_of(ipaddress.ip_network(private)) for private in ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"]):
        raise ProvisioningError("compose_network_pool_requires_private_ipv4")
    used = [ipaddress.ip_network(value) for value in occupied if ipaddress.ip_network(value).version == 4]
    allocated = {}
    for name, hosts in sorted(sizes.items(), key=lambda pair: pair[1], reverse=True):
        prefix = 32 - (hosts + 1).bit_length()
        if prefix < network.prefixlen:
            raise ProvisioningError("compose_network_pool_exhausted")
        candidate = next((part for part in network.subnets(new_prefix=prefix) if not any(part.overlaps(other) for other in used)), None)
        if candidate is None:
            raise ProvisioningError("compose_network_pool_exhausted")
        used.append(candidate)
        allocated[name] = str(candidate)
    return allocated


def private_write(path: Path, content: bytes) -> None:
    if path.is_symlink() or path.parent.is_symlink():
        raise ProvisioningError("symlink_in_provisioner_state")
    temporary = path.with_name(path.name + "." + secrets.token_hex(8) + ".tmp")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as output:
        output.write(content)
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)
    if os.name == "posix":
        descriptor = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


def run(arguments: list[str], *, data: bytes | None = None, env: dict | None = None, timeout: int = 900) -> bytes:
    # Stderr can contain resolved secrets. Never propagate it into logs or receipts.
    try:
        result = subprocess.run(arguments, input=data, capture_output=True, check=True, timeout=timeout, env=env,
                                **({"pass_fds": operator_pass_fds()} if os.name == "posix" else {}))
        return result.stdout
    except (subprocess.SubprocessError, OSError):
        # CalledProcessError/TimeoutExpired render argv, which can contain
        # credentials (for example mc alias set). Hide the exception chain too.
        raise ProvisioningError("infrastructure_command_failed:" + Path(arguments[0]).name) from None


def common_service(image: str) -> dict:
    return {"image": immutable_image(image), "restart": "unless-stopped", "read_only": True,
            "cap_drop": ["ALL"], "security_opt": ["no-new-privileges:true"], "tmpfs": ["/tmp"]}


def security_environment(config: dict) -> dict:
    profile = config.get("sandbox_security_profile", "linux_apparmor")
    if profile == "linux_apparmor":
        return {"COWORK_SANDBOX_REQUIRE_APPARMOR": "true", "COWORK_SANDBOX_APPARMOR_PROFILE": "open-cowork-sandbox"}
    if profile == "docker_desktop_seccomp":
        return {"COWORK_SANDBOX_REQUIRE_APPARMOR": "false"}
    raise ProvisioningError("unknown_sandbox_security_profile")


def verify_security_host(config: dict, command=None) -> None:
    command = command or run
    security_environment(config)
    if config.get("sandbox_security_profile") == "docker_desktop_seccomp":
        info = json.loads(command(["docker", "info", "--format", "{{json .}}" ]))
        if info.get("OperatingSystem") != "Docker Desktop" or not any("seccomp" in entry for entry in info.get("SecurityOptions", [])):
            raise ProvisioningError("docker_desktop_seccomp_profile_requires_docker_desktop_with_seccomp")


def egress_config(instance: dict) -> str:
    """Exact HTTPS host grants; no implicit public or private network access."""
    domains = []
    for origin in sorted(instance["policy"].get("allowed_network_origins", [])):
        parsed = urlsplit(origin)
        if (parsed.scheme != "https" or parsed.username or parsed.password or parsed.path or parsed.query
                or parsed.fragment or parsed.port not in [None, 443]
                or not re.fullmatch(r"[a-z0-9](?:[a-z0-9.-]{0,251}[a-z0-9])?", parsed.hostname or "")
                or any(not label or len(label) > 63 or label.startswith("-") or label.endswith("-") for label in (parsed.hostname or "").split("."))):
            raise ProvisioningError("unsupported_network_origin")
        domains.append(parsed.hostname)
    lines = ["http_port 3128", "acl CONNECT method CONNECT", "acl TLS_port port 443"]
    # Includes link-local, metadata, loopback, CGNAT, multicast, reserved and IPv6.
    # IPv6 is unavailable until equivalent resolution/rebinding tests qualify it.
    for cidr in ["0.0.0.0/8", "10.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8", "169.254.0.0/16",
                 "172.16.0.0/12", "192.0.0.0/24", "192.0.2.0/24", "192.168.0.0/16", "198.18.0.0/15",
                 "198.51.100.0/24", "203.0.113.0/24", "224.0.0.0/4", "240.0.0.0/4", "::/0"]:
        lines.append("acl blocked_dst dst " + cidr)
    lines += ["http_access deny !CONNECT", "http_access deny !TLS_port", "http_access deny blocked_dst"]
    if domains:
        lines += ["acl permitted_domain dstdomain " + " ".join(domains), "http_access allow permitted_domain"]
    lines += ["http_access deny all", "cache deny all", "access_log none", "cache_log /dev/stderr",
              "pid_filename /run/squid.pid", "pinger_enable off", "forwarded_for delete", "via off", "visible_hostname cowork-instance-egress"]
    return "\n".join(lines) + "\n"


def compose_manifest(config: dict, instance: dict, directory: Path) -> dict:
    compact = identifier(instance["id"])
    scope = "cowork-" + compact
    limits = resource_limits(instance)
    try:
        caps = resource_budget.service_caps(config)
        executor_host.settings(config)
    except (resource_budget.BudgetError, executor_host.HostError) as error:
        raise ProvisioningError(str(error)) from None
    images = config["images"]
    resources = resource_names(instance, directory)
    common = {
        "COWORK_BOOTSTRAP_TOKEN_FILE": "/run/secrets/bootstrap_token",
        "DATABASE_URL_FILE": "/run/secrets/database_url",
        "COWORK_STORAGE_MASTER_KEY_FILE": "/run/secrets/storage_master_key",
        "COWORK_S3_ENDPOINT": config["s3_endpoint"],
        "COWORK_S3_BUCKET": resources["bucket"],
        "COWORK_S3_REGION": config.get("s3_region", "us-east-1"),
        "COWORK_S3_ACCESS_KEY_FILE": "/run/secrets/s3_access_key",
        "COWORK_S3_SECRET_KEY_FILE": "/run/secrets/s3_secret_key",
        "COWORK_WEB_PUSH_ENABLED": "false",
        "COWORK_INSTANCE_ID": instance["id"],
        "COWORK_ORGANISATION_ID": instance["organisation_id"],
        "COWORK_RUNNER_URL": "http://runner:8090",
        "COWORK_RUNNER_SIGNING_KEY_FILE": "/run/secrets/runner_signing_key",
        "COWORK_SERVER_CAPABILITIES": "model.external,files,shell,git,web.fetch,browser.headless,browser.visible,desktop.linux,office.ooxml,office.libreoffice,crew.python,tool.mcp.invoke",
    }
    try:
        common.update(organisation_mfa.environment(config,instance))
        common.update({"COWORK_FLEET_KEY_FILE":"/run/secrets/fleet_key","COWORK_FLEET_MANAGEMENT_URL":config["fleet_management_url"]})
    except (ValueError,TypeError,KeyError) as error:
        raise ProvisioningError(str(error)) from None
    secret_names = ["bootstrap_token", "database_url", "storage_master_key", "s3_access_key", "s3_secret_key", "runner_signing_key"]
    epoch = execution_generation.read(instance, directory)
    if epoch is not None:
        common["COWORK_EXECUTION_EPOCH"] = epoch
        common.update(execution_runtime.environment(config, images['server']))
    api = common_service(images["server"])
    api.update({"environment": {**common, "COWORK_MODE": "api", "COWORK_LISTEN_ADDR": "0.0.0.0:8080"},
                "secrets": secret_names, "networks": {"control": None, "services": None, "routing": {"aliases": [scope + "-api"]}},
                "healthcheck": {"test": ["CMD", "curl", "--fail", "--silent", "http://127.0.0.1:8080/readyz"], "interval": "10s", "timeout": "3s", "retries": 12},
                **resource_budget.service_compose(caps['api'])})
    try:
        api['environment'].update(instance_gateway.environment(config, instance))
    except instance_gateway.GatewayError as error:
        raise ProvisioningError(str(error)) from None
    if resources["generation"]:
        boundary = json.loads((directory / "generation.json").read_text()).get("launch_not_before")
        if type(boundary) is not int or boundary <= 0:
            raise ProvisioningError("restore_authentication_boundary_missing")
        api["environment"]["COWORK_FLEET_LAUNCH_NOT_BEFORE"] = str(boundary)
    if config.get("fleet_management_url"):
        secret_names = [*secret_names, "fleet_key"]
        api["secrets"] = secret_names
        api["environment"].update({"COWORK_FLEET_KEY_FILE": "/run/secrets/fleet_key",
                                   "COWORK_FLEET_MANAGEMENT_URL": config["fleet_management_url"],
                                   "COWORK_DESKTOP_ORIGINS": api['environment'].get('COWORK_DESKTOP_ORIGINS', config.get("desktop_origins", ""))})
    worker = common_service(images["server"])
    worker.update({"environment": {**common, "COWORK_MODE": "worker"}, "secrets": secret_names,
                   "networks": ["control", "services"], **resource_budget.service_compose(caps['worker'])})
    runner = common_service(images["runner"])
    runner.update({"environment": {
        "COWORK_RUNNER_LISTEN_ADDR": "0.0.0.0:8090", "COWORK_RUNNER_INSTANCE_ID": instance["id"],
        "COWORK_RUNNER_SIGNING_KEY_FILE": "/run/secrets/runner_signing_key",
        "COWORK_RUNNER_CORE_IMAGE": immutable_image(images["sandbox_core"]),
        "COWORK_RUNNER_ADMISSION_DIRECTORY": "/var/lib/cowork/admission",
        "COWORK_RUNNER_CPU_MILLIS": str(limits["cpu_millis"]),
        "COWORK_RUNNER_MEMORY_MIB": str(limits["memory_mib"]),
        "COWORK_RUNNER_CONCURRENT_RUNS": str(limits["concurrent_runs"]),
        "COWORK_RUNNER_GUI_IMAGE": immutable_image(images["sandbox_gui"]),
        "COWORK_RUNNER_CREW_IMAGE": immutable_image(images["sandbox_crew"]),
        "COWORK_SANDBOX_EGRESS_NETWORK": scope + "-sandbox", "COWORK_SANDBOX_HTTP_PROXY": "http://egress-proxy:3128",
        **security_environment(config),
    }, "secrets": ["runner_signing_key"], "volumes": ["/var/run/docker.sock:/var/run/docker.sock", "runner-admission:/var/lib/cowork/admission"], "networks": ["control"],
    "healthcheck": {"test": ["CMD", "wget", "--spider", "-q", "http://127.0.0.1:8090/healthz"], "interval": "5s", "timeout": "3s", "retries": 20},
    **resource_budget.service_compose(caps['runner'])})
    if resources["generation"]:
        runner["environment"]["COWORK_RUNNER_STORAGE_GENERATION"] = resources["generation"]
    proxy = common_service(images["egress_proxy"])
    proxy.update({"networks": {"sandbox": {"aliases": ["egress-proxy"]}, "internet": None},
                  "configs": [{"source": "egress_policy", "target": "/etc/squid/squid.conf", "mode": 0o444}],
                  "healthcheck": {"test": ["CMD", "curl", "--silent", "--max-time", "2", "--output", "/dev/null", "http://127.0.0.1:3128"], "interval": "5s", "timeout": "3s", "retries": 12},
                  "tmpfs": ["/run:uid=13,gid=13,mode=0755", "/var/log/squid:uid=13,gid=13,mode=0755", "/var/spool/squid:uid=13,gid=13,mode=0755"],
                  **resource_budget.service_compose(caps['egress-proxy'])})
    manifest = {"name": scope, "services": {"api": api, "worker": worker, "runner": runner, "egress-proxy": proxy},
            "volumes": {"runner-admission": {"name": scope + "-runner-admission"}},
            "networks": {"control": {"internal": True}, "sandbox": {"internal": True, "name": scope + "-sandbox"},
                         "internet": {}, "services": {"external": True, "name": config["services_network"]},
                         "routing": {"external": True, "name": config["routing_network"]}},
            "secrets": {key: {"file": str(directory / "runtime-secrets" / key)} for key in secret_names},
            "configs": {"egress_policy": {"file": str(directory / "egress.conf")}}}
    for name, subnet in config.get("compose_subnets", {}).items():
        if name not in ["control", "sandbox", "internet"]:
            raise ProvisioningError("invalid_instance_network_name")
        if subnet:
            manifest["networks"][name]["ipam"] = {"config": [{"subnet": str(ipaddress.ip_network(subnet))}]}
    try:
        manifest = guest_endpoints.apply_manifest(config, manifest, directory)
        manifest = instance_disk_quotas.apply_configuration(config, instance, directory, manifest)
        instance_gateway.validate_manifest(config, instance, manifest)
        return manifest
    except (guest_endpoints.GuestEndpointError, instance_gateway.GatewayError) as error:
        raise ProvisioningError(str(error)) from None


class Infrastructure:
    private_write = staticmethod(private_write)
    resource_names = staticmethod(resource_names)

    def __init__(self, config: dict):
        self.config = config
        root = Path(config["state_directory"])
        if not root.is_absolute() or root.is_symlink() or root.resolve() == Path(root.anchor):
            raise ProvisioningError("unsafe_state_directory")
        root.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.root = root.resolve()
        try:
            resource_budget.service_caps(config)
            resource_budget.host_budget(config)
            host_capacity_attestation.settings(config)
            remote_lifecycle_qualification.settings(config)
            instance_gateway.settings(config)
            self.host = executor_host.ExecutorHost(self, lambda args, **kwargs: run(args, **kwargs))
            backend_profile.settings(config)
            if config.get('backend')=='kubernetes' and config.get('backend_profile') is None:
                raise ProvisioningError('kubernetes_original_backend_profile_required')
        except (resource_budget.BudgetError, executor_host.HostError, host_capacity_attestation.CapacityError,
                remote_lifecycle_qualification.LifecycleError, instance_gateway.GatewayError,backend_profile.ProfileError) as error:
            raise ProvisioningError(str(error)) from None

    def run(self, arguments, **kwargs):
        try:
            instance_disk_quotas.command_gate(self,arguments)
            if arguments and self.config.get('backend')=='kubernetes' and arguments[0] in ('kubectl','helm'):
                config_path=Path(self.config['kubeconfig_file'])
                if not config_path.is_absolute() or config_path.is_symlink() or not config_path.is_file() or any(p.is_symlink() for p in config_path.parents):
                    raise ProvisioningError('unsafe_original_kubernetes_configuration')
                if any(a in ('--kubeconfig','--context','--kube-context','--server','--kube-apiserver','--token','--insecure-skip-tls-verify')
                       or a.startswith(('--kubeconfig=','--context=','--kube-context=','--server=','--kube-apiserver=','--token=','--insecure-skip-tls-verify=')) for a in arguments[1:]):
                    raise ProvisioningError('kubernetes_original_endpoint_override_forbidden')
                selected_context=self.config['kubernetes_context']
                if not isinstance(selected_context,str) or not selected_context or '\0' in selected_context:
                    raise ProvisioningError('invalid_original_kubernetes_context')
                arguments=[arguments[0],'--kubeconfig',str(config_path),'--context' if arguments[0]=='kubectl' else '--kube-context',selected_context,*arguments[1:]]
                selected_environment=dict(os.environ if kwargs.get('env') is None else kwargs['env'])
                for key in list(selected_environment):
                    if key=='KUBECONFIG' or key.startswith('HELM_KUBE'):
                        selected_environment.pop(key,None)
                kwargs['env']=selected_environment
            if arguments and arguments[0] == 'docker':
                self.host.bind()
                self.host.verify()
                kwargs['env'] = self.host.environment(kwargs.get('env'))
                arguments = self.host.arguments(arguments)
            return run(arguments, **kwargs)
        except executor_host.HostError as error:
            raise ProvisioningError(str(error)) from None

    def directory(self, instance_id: str) -> Path:
        try:
            self.host.bind()
            if self.config.get('backend')=='kubernetes' and self.config.get('backend_profile') is not None:
                selected=backend_profile.settings(self.config)
                if selected['specification']['instance_id']!=instance_id:
                    raise ProvisioningError('kubernetes_operator_state_scope_differs')
                marker=self.root/'kubernetes-profile.json'
                if marker.exists():
                    if json.loads(marker.read_bytes())!=selected['specification']:
                        raise ProvisioningError('kubernetes_operator_profile_retarget_requires_review')
                else:
                    private_write(marker,json.dumps(selected['specification'],sort_keys=True).encode())
            else:
                instance_disk_quotas.bind(self)
                resource_budget.bind_configuration(self)
                guest_endpoints.bind(self)
                remote_lifecycle_qualification.bind(self)
        except (executor_host.HostError, resource_budget.BudgetError, guest_endpoints.GuestEndpointError,
                remote_lifecycle_qualification.LifecycleError) as error:
            raise ProvisioningError(str(error)) from None
        path = self.root / identifier(instance_id)
        if path.is_symlink() or not path.resolve().is_relative_to(self.root):
            raise ProvisioningError("instance_path_escape")
        path.mkdir(mode=0o700, exist_ok=True)
        try:
            self.host.bind(path)
        except executor_host.HostError as error:
            raise ProvisioningError(str(error)) from None
        return path

    def active_directory(self, directory: Path) -> Path:
        pointer = directory / "active.json"
        if not pointer.exists():
            return directory
        if pointer.is_symlink():
            raise ProvisioningError("unsafe_generation_pointer")
        generation = json.loads(pointer.read_text())["generation"]
        target = directory / "generations" / identifier(generation)
        if target.is_symlink() or not target.resolve().is_relative_to(directory.resolve()) or not target.is_dir():
            raise ProvisioningError("unsafe_generation_pointer")
        if json.loads((target / "generation.json").read_text())["generation"] != generation:
            raise ProvisioningError("generation_pointer_mismatch")
        return target

    def validate_host_operation(self, operation, directory):
        try:
            backend_profile.validate(self,operation,physical=False)
            self.host.prepare_operation(operation, directory)
            host_capacity_attestation.validate_operation(self, operation)
        except (executor_host.HostError, host_capacity_attestation.CapacityError,backend_profile.ProfileError) as error:
            raise ProvisioningError(str(error)) from None

    def ensure_secrets(self, instance: dict, directory: Path) -> dict:
        try:
            guest_endpoints.require_instance(self.config, instance)
        except guest_endpoints.GuestEndpointError as error:
            raise ProvisioningError(str(error)) from None
        operator_secrets = getattr(self, '_private_operator_secrets', None)
        epoch = execution_generation.read(instance, directory)
        if epoch is None and instance.get('execution_required') is True:
            raise ProvisioningError('execution_generation_marker_required_for_fleet_key')
        if self.host.value is not None:
            if (not self.config.get('fleet_management_url') or operator_secrets is None
                    or operator_secrets['instance_id'] != instance['id']
                    or operator_secrets['organisation_id'] != instance['organisation_id']
                    or operator_secrets.get('execution_epoch') != epoch
                    or operator_secrets['executor_host_binding'] != executor_host.manager_binding(self.config)):
                raise ProvisioningError('hosted_instance_operator_key_required')
        secret_dir = directory / "secrets"
        if secret_dir.is_symlink():
            raise ProvisioningError("secret_path_escape")
        secret_dir.mkdir(mode=0o700, exist_ok=True)
        values = {}
        for key in ["bootstrap_token", "database_password", "runner_signing_key", "storage_master_key", "s3_access_key", "s3_secret_key"]:
            path = secret_dir / key
            if path.is_symlink():
                raise ProvisioningError("secret_path_escape")
            if not path.exists():
                value = base64.b64encode(secrets.token_bytes(32)).decode() if key == "storage_master_key" else secrets.token_hex(32)
                private_write(path, value.encode())
            values[key] = path.read_text().strip()
        db = resource_names(instance, directory)["database"]
        connection = self.config["database_endpoint"]
        if not re.fullmatch(r"[a-zA-Z0-9.-]+:[0-9]+", connection):
            raise ProvisioningError("invalid_database_endpoint")
        try:
            values["database_url"] = guest_endpoints.database_url(self.config, f"postgres://{db}:{values['database_password']}@{connection}/{db}")
        except guest_endpoints.GuestEndpointError as error:
            raise ProvisioningError(str(error)) from None
        private_write(secret_dir / "database_url", values["database_url"].encode())
        if self.config.get("fleet_management_url"):
            if self.host.value is not None:
                values['fleet_key'] = operator_secrets['fleet_key']
            else:
                key = Path(self.config["provisioner_token_file"]).read_text().strip()
                if len(key) < 32:
                    raise ProvisioningError("invalid_provisioner_token")
                payload = (f"cowork-federation-v2\0{instance['organisation_id']}\0{instance['id']}\0{epoch}"
                           if epoch is not None else f"cowork-federation-v1\0{instance['organisation_id']}\0{instance['id']}")
                values["fleet_key"] = hmac.new(key.encode(), payload.encode(), hashlib.sha256).hexdigest()
            private_write(secret_dir / 'fleet_key', values['fleet_key'].encode())
        # Compose file secrets retain source permissions. Their parent is 0700
        # on the host, while mounted copies must be readable by container UID 10001.
        runtime_secrets = directory / "runtime-secrets"
        if runtime_secrets.is_symlink():
            raise ProvisioningError("secret_path_escape")
        runtime_secrets.mkdir(mode=0o700, exist_ok=True)
        for key in ["bootstrap_token", "database_url", "runner_signing_key", "storage_master_key", "s3_access_key", "s3_secret_key", *(["fleet_key"] if "fleet_key" in values else [])]:
            private_write(runtime_secrets / key, values[key].encode())
            if os.name == "posix":
                (runtime_secrets / key).chmod(0o444)
        try:
            guest_endpoints.copy_public_ca(self.config, directory, private_write)
        except guest_endpoints.GuestEndpointError as error:
            raise ProvisioningError(str(error)) from None
        instance_disk_quotas.runtime_secrets(self,instance,directory)
        return values

    def stop_compose(self, instance: dict, directory: Path, *, preserve_data_services=True) -> None:
        # Stop producers first, then every surviving runtime container owned by
        # this instance. Docker containers outlive a stopped runner process.
        dedicated=self.config.get('instance_disk_quotas') is not None
        self.compose(instance, directory, "stop", *(['api','worker','runner','egress-proxy'] if dedicated and preserve_data_services else []))
        prefix = "cowork-" + identifier(instance["id"]) + "-"
        runtime_pattern = re.escape(prefix) + r"(?:(?:job|desktop|terminal|transfer)-[0-9a-f]{32}|(?:g-[0-9a-f]{32}-)?snapshot-[0-9a-f]{32})"
        running = self.run(["docker", "ps", "--filter", "name=^/" + prefix, "--format", "{{.Names}}"]).decode().splitlines()
        for name in running:
            if dedicated and preserve_data_services and re.fullmatch(re.escape(prefix)+r'(?:postgres|minio)-1',name):
                continue
            if not re.fullmatch(runtime_pattern, name):
                raise ProvisioningError("unexpected_instance_runtime_container")
            self.run(["docker", "stop", "--time", "20", name])
        names = self.run(["docker", "ps", "--all", "--filter", "name=^/" + prefix, "--format", "{{.Names}}"]).decode().splitlines()
        for name in names:
            if re.fullmatch(re.escape(prefix) + (r"(?:api|worker|runner|egress-proxy|postgres|minio)-[1-9][0-9]*" if dedicated else r"(?:api|worker|runner|egress-proxy)-[1-9][0-9]*"), name):
                # Compose services remain stopped, with their volumes intact.
                continue
            if not re.fullmatch(runtime_pattern, name):
                raise ProvisioningError("unexpected_instance_runtime_container")
            # Include acknowledged-but-not-started Docker creates. Their durable
            # admission reservations can be released only once the ID is gone.
            self.run(["docker", "rm", "--force", name])
        remaining = self.run(["docker", "ps", "--filter", "name=^/" + prefix, "--format", "{{.Names}}"])
        if dedicated and preserve_data_services:
            if any(not re.fullmatch(re.escape(prefix)+r'(?:postgres|minio)-1',n) for n in remaining.decode().splitlines()):
                raise ProvisioningError("instance_workloads_not_quiescent")
        elif remaining.strip():
            raise ProvisioningError("instance_workloads_not_quiescent")

    def verify_stopped(self, instance: dict, active: Path) -> None:
        if self.config.get('instance_disk_quotas') is not None:
            import instance_disk_quotas
            if instance_disk_quotas.data_services_stopped(self):return
        resources = resource_names(instance, active)
        env = pg_environment(Path(self.config["database_admin_url_file"]).read_text().strip(), self.config)
        connections = self.run(["psql", "-X", "-tAc", f"SELECT count(*) FROM pg_stat_activity WHERE datname='{resources['database']}';"], env=env)
        if connections.strip() != b"0":
            raise ProvisioningError("instance_database_not_quiescent")

    def ensure_database(self, instance: dict, values: dict, directory: Path) -> None:
        db = resource_names(instance, directory)["database"]
        env = pg_environment(Path(self.config['database_admin_url_file']).read_text().strip(), self.config)
        # UUID-derived identifiers and generated hex secrets, never user-provided SQL.
        query = f"SELECT 1 FROM pg_roles WHERE rolname='{db}';"
        if self.run(["psql", "-X", "-tAc", query], env=env).strip() != b"1":
            self.run(["psql", "-X", "-v", "ON_ERROR_STOP=1"], data=f"CREATE ROLE {db} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE PASSWORD '{values['database_password']}';".encode(), env=env)
        exists = self.run(["psql", "-X", "-tAc", f"SELECT 1 FROM pg_database WHERE datname='{db}';"], env=env).strip()
        if exists != b"1":
            self.run(["psql", "-X", "-v", "ON_ERROR_STOP=1"], data=f"CREATE DATABASE {db} OWNER {db} TEMPLATE template0; REVOKE ALL ON DATABASE {db} FROM PUBLIC;".encode(), env=env)

    def ensure_bucket(self, instance: dict, values: dict, directory: Path) -> None:
        # The operator configures an mc alias in a private config directory; it is
        # never supplied by an organisation or included in operation responses.
        alias = self.config["s3_admin_alias"]
        if not re.fullmatch(r"[a-zA-Z][a-zA-Z0-9_-]*", alias):
            raise ProvisioningError("invalid_s3_alias")
        bucket = resource_names(instance, directory)["bucket"]
        command = ["mc", "--config-dir", self.config["s3_admin_config_directory"]]
        try:
            environment = guest_endpoints.controller_storage_environment(self.config)
        except guest_endpoints.GuestEndpointError as error:
            raise ProvisioningError(str(error)) from None
        arguments = {'env': environment} if environment is not None else {}
        self.run(command + ["mb", "--ignore-existing", f"{alias}/{bucket}"], **arguments)
        self.run(command + ["anonymous", "set", "none", f"{alias}/{bucket}"], **arguments)
        policy = {"Version": "2012-10-17", "Statement": [
            {"Effect": "Allow", "Action": ["s3:ListBucket", "s3:GetBucketLocation"], "Resource": [f"arn:aws:s3:::{bucket}"]},
            {"Effect": "Allow", "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:AbortMultipartUpload", "s3:ListMultipartUploadParts"], "Resource": [f"arn:aws:s3:::{bucket}/*"]}]}
        policy_file = directory / "bucket-policy.json"
        private_write(policy_file, json.dumps(policy).encode())
        # mc user add accepts secret arguments; use a restricted temporary config
        # and private process namespace for the provisioner, never a workload pod.
        self.run(command + ["admin", "user", "add", alias, values["s3_access_key"], values["s3_secret_key"]], **arguments)
        self.run(command + ["admin", "policy", "create", alias, bucket, str(policy_file)], **arguments)
        self.run(command + ["admin", "policy", "attach", alias, bucket, "--user", values["s3_access_key"]], **arguments)

    def compose(self, instance: dict, directory: Path, *arguments: str) -> bytes:
        arguments=instance_disk_quotas.compose_arguments(self.config,arguments)
        active = self.active_directory(directory)
        if arguments and arguments[0] in ('up', 'start', 'run', 'restart'):
            try:
                instance_gateway.validate_manifest(self.config, instance, json.loads((directory / 'compose.json').read_bytes()))
            except instance_gateway.GatewayError as error:
                raise ProvisioningError(str(error)) from None
        private_write(active / "egress.conf", egress_config(instance).encode())
        (active / "egress.conf").chmod(0o444)
        return self.run(["docker", "compose", "--project-name", "cowork-" + identifier(instance["id"]), "--file", str(directory / "compose.json"), *arguments])

    def write_compose(self, instance: dict, directory: Path) -> None:
        active = self.active_directory(directory)
        # A normal start is not an upgrade. Keep the previously qualified images,
        # even when the operator's defaults have moved to a newer release.
        pinned = active / "images.json"
        if pinned.exists():
            images = json.loads(pinned.read_text())
        elif (directory / "compose.json").exists():
            images = self.deployed_images(directory)
        else:
            images = self.config["images"]
        images = {name: immutable_image(value) for name, value in images.items()}
        execution_runtime.probe(self, instance, active, images)
        organisation_mfa.probe(self,instance,images["server"])
        private_write(pinned, json.dumps(images).encode())
        subnets = self.network_plan(instance, directory)
        try:
            service_limits = resource_budget.read(directory / 'resource-budget.json')['services'] if (directory / 'resource-budget.json').exists() else resource_budget.service_caps(self.config)
            manifest = compose_manifest({**self.config, "images": images, "compose_subnets": subnets,
                                         'instance_service_limits': service_limits}, instance, active)
            self.host.verify_shared_state(images['sandbox_core'])
            resource_budget.reserve(self, instance, directory, manifest)
        except (executor_host.HostError, resource_budget.BudgetError, KeyError, TypeError, ValueError) as error:
            code = str(error) if isinstance(error, (executor_host.HostError, resource_budget.BudgetError)) else 'invalid_resource_budget_evidence'
            raise ProvisioningError(code) from None
        private_write(directory / "compose.json", json.dumps(manifest).encode())

    def network_plan(self, instance: dict, directory: Path) -> dict:
        pool = self.config.get("compose_network_pool")
        if not pool:
            return {}
        marker = directory / "networks.json"
        if marker.exists():
            subnets = json.loads(marker.read_text())
            if subnets.get("sandbox") and ipaddress.ip_network(subnets["sandbox"]).num_addresses - 2 < resource_limits(instance)["concurrent_runs"] + 8:
                raise ProvisioningError("instance_network_capacity_requires_migration")
            return subnets
        ids = self.run(["docker", "network", "ls", "--quiet"]).decode().splitlines()
        networks = json.loads(self.run(["docker", "network", "inspect", *ids])) if ids else []
        occupied = [entry["Subnet"] for network in networks for entry in network.get("IPAM", {}).get("Config", []) or [] if entry.get("Subnet")]
        for path in self.root.glob("*/networks.json"):
            occupied += [value for value in json.loads(path.read_text()).values() if value]
        scope = "cowork-" + identifier(instance["id"])
        names = {"control": scope + "_control", "internet": scope + "_internet", "sandbox": scope + "-sandbox"}
        # Preserve legacy Docker-assigned networks without forcing recreation.
        existing = {kind: None for kind, name in names.items() if any(network["Name"] == name for network in networks)}
        sizes = {"control": 8, "internet": 8, "sandbox": resource_limits(instance)["concurrent_runs"] + 8}
        subnets = {**existing, **allocate_subnets(pool, {kind: size for kind, size in sizes.items() if kind not in existing}, occupied)}
        private_write(marker, json.dumps(subnets).encode())
        return subnets

    @staticmethod
    def deployed_images(directory: Path) -> dict:
        services = json.loads((directory / "compose.json").read_text())["services"]
        return {"server": services["api"]["image"], "runner": services["runner"]["image"],
                "egress_proxy": services["egress-proxy"]["image"], **{
                    name: services["runner"]["environment"][key] for name, key in [
                        ("sandbox_core", "COWORK_RUNNER_CORE_IMAGE"), ("sandbox_gui", "COWORK_RUNNER_GUI_IMAGE"), ("sandbox_crew", "COWORK_RUNNER_CREW_IMAGE")]}}

    def execute(self, operation: dict, *, enrollment_resume=False) -> dict:
        try:
            self._private_operator_secrets = executor_host.operator_secrets(self.config, operation)
        except executor_host.HostError as error:
            raise ProvisioningError(str(error)) from None
        try:
            result=self._execute_operation(operation, enrollment_resume=enrollment_resume)
            return backend_profile.attach(operation,result)
        except backend_profile.ProfileError as error:
            raise ProvisioningError(str(error)) from None
        finally:
            # A later operation must bring its own fresh, scope-checked envelope.
            self._private_operator_secrets = None
            self._disk_current_operation = None

    def _execute_operation(self, operation: dict, *, enrollment_resume=False) -> dict:
        instance = operation["instance"]
        identifier(instance["organisation_id"])
        directory = self.directory(instance["id"])
        operation_id = identifier(operation["id"])
        self.validate_host_operation(operation, directory)
        receipt_path = directory / ("receipt-" + operation_id + ".json")
        if receipt_path.exists():
            receipt = json.loads(receipt_path.read_text())
            backend_profile.validate_receipt(self.config,operation,receipt)
            execution_generation.validate_receipt(operation,receipt)
            execution_recovery.validate_receipt(operation,receipt)
            instance_enrollment.validate_receipt(operation,receipt)
            provisioning_release.validate_receipt(operation,receipt)
            executor_host.validate_receipt(operation,receipt)
            host_capacity_attestation.validate_receipt(operation,receipt)
            if operation.get('action') in ['import_quiesce','import_export','import_restore','import_qualify']:
                import instance_imports
                instance_imports.validate_receipt(operation,receipt)
            return receipt
        if instance['backend']=='kubernetes' and operation.get('backend_profile') is not None:
            import kubernetes_lifecycle
            recovered=kubernetes_lifecycle.recover_completed(self,operation,directory)
            if recovered is not None:
                recovered=backend_profile.attach(operation,executor_host.attach_receipt(operation,recovered))
                recovered=host_capacity_attestation.attach_receipt(operation,recovered)
                backend_profile.serving(self,operation)
                private_write(receipt_path,json.dumps(recovered).encode())
                return recovered
            if enrollment_resume and kubernetes_lifecycle.partial_deletion_pending(self,operation,directory):
                receipt=kubernetes_lifecycle.execute(self,operation,directory,enrollment_resume=True)
                receipt=backend_profile.attach(operation,executor_host.attach_receipt(operation,receipt))
                receipt=host_capacity_attestation.attach_receipt(operation,receipt)
                private_write(receipt_path,json.dumps(receipt).encode())
                return receipt
        try:
            backend_profile.validate(self,operation)
            instance_disk_quotas.authorize_operation(self,operation)
        except backend_profile.ProfileError as error:
            raise ProvisioningError(str(error)) from None
        try:
            host_capacity_attestation.validate_operation(self, operation, activating=True)
        except host_capacity_attestation.CapacityError as error:
            raise ProvisioningError(str(error)) from None
        if instance["backend"] not in ["compose", "kubernetes"]:
            raise ProvisioningError("unsupported_deployment_backend")
        action = operation["action"]
        provisioning_release.selection(operation)
        if action=='provision' and instance.get('provisioning_release') is not None and self.config.get('release_selection_enabled') is not True:
            raise ProvisioningError('provisioning_release_profile_unavailable')
        if instance['backend']=='kubernetes' and operation.get('backend_profile') is not None:
            import kubernetes_lifecycle
            receipt=kubernetes_lifecycle.execute(self,operation,directory,enrollment_resume=enrollment_resume)
            receipt=backend_profile.attach(operation,executor_host.attach_receipt(operation,receipt))
            receipt=host_capacity_attestation.attach_receipt(operation,receipt)
            backend_profile.serving(self,operation)
            private_write(receipt_path,json.dumps(receipt).encode())
            return receipt
        recovery = execution_recovery.binding(operation)
        execution_generation.prepare(self, operation, directory)
        if action.startswith('import_'):
            import instance_imports
            receipt = instance_imports.execute(self, operation, directory)
            receipt = execution_generation.attach(self, operation, receipt, True)
            receipt = executor_host.attach_receipt(operation, receipt)
            receipt = host_capacity_attestation.attach_receipt(operation, receipt)
            receipt=backend_profile.attach(operation,receipt)
            self.private_write(receipt_path, json.dumps(receipt).encode())
            return receipt
        if action == "delete":
            try:
                receipt = InstanceDeletion(self).execute(operation, directory)
            except (DeletionError, ValueError, KeyError, OSError) as error:
                raise ProvisioningError("instance_deletion_requires_review") from None
            receipt = execution_generation.attach(self,operation,receipt,True)
            receipt = executor_host.attach_receipt(operation,receipt)
            receipt = host_capacity_attestation.attach_receipt(operation,receipt)
            receipt=backend_profile.attach(operation,receipt)
            private_write(receipt_path, json.dumps(receipt).encode())
            return receipt
        if (directory / "deleted.json").exists():
            raise ProvisioningError("instance_permanently_deleted")
        active = self.active_directory(directory)
        receipt = {"receipt_id": operation["id"]}
        if action == "provision":
            # Strong isolation must not be represented by the Docker-only runner.
            if instance["policy"]["minimum_isolation"] == "virtual_machine" and not backend_profile.vm_instance(self.config,instance):
                raise ProvisioningError("vm_instance_runner_not_qualified")
            verify_security_host(self.config, self.run)
            try:
                selected_images = provisioning_release.prepare(self,operation,directory,active)
            except ReleaseError as error:
                raise ProvisioningError(str(error)) from None
            if instance["backend"] == "compose":
                images = selected_images or (self.deployed_images(directory) if (directory / "compose.json").exists() else self.config["images"])
                execution_runtime.probe(self, instance, active, images)
                retain_fleet_images(images, self.run, immutable_image)
            values = self.ensure_secrets(instance, active)
            self.ensure_database(instance, values, active)
            self.ensure_bucket(instance, values, active)
            if instance["backend"] == "compose":
                self.write_compose(instance, directory)
                self.compose(instance, directory, "up", "--detach", "--wait", "--wait-timeout", "180")
            else:
                self.helm(instance, directory, values)
        elif action in ["start", "resume", "stop", "suspend"]:
            if instance["backend"] == "compose":
                if action in ["start", "resume"]:
                    if instance["policy"]["minimum_isolation"] == "virtual_machine" and not backend_profile.vm_instance(self.config,instance):
                        raise ProvisioningError("vm_instance_runner_not_qualified")
                    verify_security_host(self.config, self.run)
                    retain_fleet_images(self.deployed_images(directory), self.run, immutable_image)
                    self.ensure_secrets(instance, active)
                    self.write_compose(instance, directory)
                    self.compose(instance, directory, "up", "--detach", "--wait")
                else:
                    if recovery is not None:
                        receipt['execution_recovery']=execution_recovery.stop(self,operation,directory)
                    else:
                        self.stop_compose(instance, directory)
            else:
                raise ProvisioningError('kubernetes_original_backend_profile_required')
        elif action == 'enroll_execution':
            receipt.update(instance_enrollment.execute(self,operation,directory,resume=enrollment_resume))
        elif action in ["upgrade", "rollback"]:
            verify_security_host(self.config, self.run)
            try:
                receipt.update((InstanceRollbacks(self) if action == "rollback" else InstanceUpdates(self)).execute(operation, directory))
            except ReleaseError as error:
                raise ProvisioningError(str(error), error.instance_state, error.result) from error
        elif action in ["backup", "restore"]:
            verify_security_host(self.config, self.run)
            manager = None
            before = {name: (directory / name).read_bytes() if (directory / name).exists() else None for name in ["active.json", "compose.json"]}
            try:
                manager = InstanceBackups(self)
                receipt.update(manager.execute(operation, directory))
            except (BackupError, ProvisioningError) as error:
                safe_state = None
                if action == "restore" and manager is not None and manager.quiesced and all(
                        ((directory / name).read_bytes() if (directory / name).exists() else None) == value for name, value in before.items()):
                    # Failed staging never replaced the original resources. Check
                    # that no helper/workload survived before allowing a restart.
                    manager.quiescent(instance, active)
                    safe_state = "stopped"
                raise ProvisioningError(str(error), safe_state) from error
        else:
            raise ProvisioningError("operation_not_qualified:" + action)
        receipt = provisioning_release.attach(self,operation,directory,receipt)
        receipt = execution_generation.attach(self,operation,receipt,True)
        receipt = executor_host.attach_receipt(operation,receipt)
        receipt = host_capacity_attestation.attach_receipt(operation,receipt)
        receipt = backend_profile.attach(operation,receipt)
        backend_profile.serving(self,operation)
        private_write(receipt_path, json.dumps(receipt).encode())
        return receipt

    def helm(self, instance: dict, directory: Path, secrets_map: dict) -> None:
        namespace = "cowork-" + identifier(instance["id"])
        # Kubernetes workers require a qualified runner endpoint; never mount a
        # Docker socket on a Kubernetes node as a hidden compatibility fallback.
        runner = self.config.get("kubernetes_runner_url")
        if not runner:
            raise ProvisioningError("kubernetes_runner_not_configured")
        mfa_environment=organisation_mfa.environment(self.config,instance)
        organisation_mfa.probe(self,instance,immutable_image(self.config["images"]["server"]))
        values = {"instanceId": instance["id"], "organisationId": instance["organisation_id"],
                  "image": immutable_image(self.config["images"]["server"]), "replicas": 1,
                  "serviceResources": {name: {key: value[key] for key in ['cpu_millis', 'memory_mib']}
                                       for name, value in resource_budget.service_caps(self.config).items() if name in ['api', 'worker']},
                  "secrets": secrets_map, "s3Endpoint": self.config["s3_endpoint"],
                  "s3Bucket": namespace, "runnerUrl": runner, "resources": resource_limits(instance),
                  "serviceCidrs": self.config.get("kubernetes_service_cidrs", []),
                  "routingNamespace": self.config.get("kubernetes_routing_namespace", "cowork-management"),
                  "organisationMfa":organisation_mfa.configuration(instance),
                  "fleetManagementUrl":self.config.get("fleet_management_url")}
        if not values["serviceCidrs"]:
            raise ProvisioningError("kubernetes_service_cidrs_not_configured")
        values_file = directory / "helm-values.json"
        private_write(values_file, json.dumps(values).encode())
        self.run(["helm", "upgrade", "--install", namespace, self.config["instance_chart"], "--namespace", namespace,
             "--create-namespace", "--values", str(values_file), "--atomic", "--wait", "--timeout", "5m"])


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ProvisioningError("management_redirect_rejected")


class Manager:
    def __init__(self, config: dict):
        self.url = config["management_url"].rstrip("/")
        parsed = urlsplit(self.url)
        if parsed.scheme != "https" and not (parsed.scheme == "http" and parsed.hostname in ["localhost", "127.0.0.1", "::1"]):
            raise ProvisioningError("management_connection_requires_tls")
        if parsed.username or parsed.password or parsed.query or parsed.fragment:
            raise ProvisioningError("invalid_management_url")
        self.token = Path(config["provisioner_token_file"]).read_text().strip()
        if len(self.token) < 32:
            raise ProvisioningError("invalid_provisioner_token")
        ca=config.get('management_ca_file')
        self.http = build_opener(ProxyHandler({}), NoRedirect(), HTTPSHandler(context=ssl.create_default_context(cafile=ca)))

    def call(self, path: str, payload: dict):
        request = Request(self.url + "/management/runner/v1" + path,
                          json.dumps(payload).encode(), {"Content-Type": "application/json", "x-cowork-provisioner-token": self.token})
        with self.http.open(request, timeout=30) as response:
            content = response.read(32 * 1024 * 1024 + 1)
            if len(content) > 32 * 1024 * 1024:
                raise ProvisioningError('management_response_exceeds_limit')
            return json.loads(content)


def deliver_completion(manager, operation, completion):
    # A retry only delivers the exact stored result. It never executes again.
    for attempt in range(8):
        try:
            manager.call(f"/operations/{operation['id']}/complete", completion)
            attestor = getattr(manager, 'capacity_attestor', None)
            if isinstance(attestor, host_capacity_attestation.CapacityAttestor) and attestor.configured is not None:
                try:
                    attestor.refresh()
                except host_capacity_attestation.CapacityError:
                    pass  # Completion committed; renewal status/manager TTL fence new work.
            return
        except OSError:
            if attempt == 7:
                raise
            time.sleep(min(2 ** attempt, 30))


def reconcile_operations(manager, infrastructure):
    # Called while holding the exclusive operator lock. Pending does not claim,
    # renew or reassign an execution lease.
    for operation in manager.call("/pending", operator_contract(infrastructure.config)):
        try:
            journal = OperationJournal(infrastructure, operation)
            completion = journal.recover()
            if completion is None and operation.get("recovery_authorized"):
                if operation['instance']['backend']=='kubernetes' and operation.get('backend_profile') is not None:
                    receipt=infrastructure.execute(operation,enrollment_resume=True)
                    completion=journal.finish(True,receipt)
                    deliver_completion(manager,operation,completion)
                    continue
                if operation.get('backend_profile') is not None:
                    backend_profile.validate(infrastructure,operation,physical=True)
                    host_capacity_attestation.validate_operation(infrastructure,operation,activating=True)
                if operation['action']=='enroll_execution':
                    try:
                        receipt=infrastructure.execute(operation,enrollment_resume=True)
                        completion=journal.finish(True,receipt)
                    except ProvisioningError as error:
                        if error.result.get('execution_enrollment',{}).get('outcome')!='unchanged':raise
                        completion=journal.finish(False,{**error.result,'error_code':str(error),'instance_state':'stopped'})
                    deliver_completion(manager,operation,completion)
                    continue
                if execution_recovery.binding(operation) is not None:
                    directory=infrastructure.directory(operation['instance_id'])
                    proof=execution_recovery.stop(infrastructure,operation,directory)
                    completion=journal.finish(True,{'receipt_id':operation['id'],'execution_recovery':proof})
                    deliver_completion(manager,operation,completion)
                    continue
                if operation["action"] == "delete":
                    receipt = InstanceDeletion(infrastructure).execute(operation, infrastructure.directory(operation["instance_id"]), resume=True)
                    completion = journal.finish(True, receipt)
                    deliver_completion(manager, operation, completion)
                    continue
                if operation["action"] in ["upgrade", "rollback"]:
                    backup = InstanceUpdates(infrastructure).recover(operation)
                    completion = journal.finish(False, {**backup, "error_code": "operation_interrupted_resources_stopped", "instance_state": "stopped"})
                    deliver_completion(manager, operation, completion)
                    continue
                instance = operation["instance"]
                if instance["backend"] != "compose" or not (instance["policy"]["minimum_isolation"] == "container" or backend_profile.vm_instance(infrastructure.config,instance)):
                    raise JournalError("recovery_backend_not_qualified")
                directory = infrastructure.directory(instance["id"])
                active = infrastructure.active_directory(directory)
                # Missing/invalid manifests require operator review, not an
                # invented startable state for a partially provisioned instance.
                infrastructure.deployed_images(directory)
                infrastructure.stop_compose(instance, directory)
                infrastructure.verify_stopped(instance, active)
                provisioning_release.verify_installed(infrastructure,operation,directory)
                infrastructure.write_compose(instance, directory)
                stopped = "suspended" if operation["action"] == "suspend" or instance["state"] == "suspended" else "stopped"
                completion = journal.finish(False, {"error_code": "operation_interrupted_resources_stopped", "instance_state": stopped})
            if completion is not None:
                deliver_completion(manager, operation, completion)
                continue
            reason = "operation_completion_unproven"
        except (JournalError, ProvisioningError, BackupError, ReleaseError, DeletionError, ExecutionGenerationError, executor_host.HostError, host_capacity_attestation.CapacityError, backend_profile.ProfileError, ValueError, KeyError, OSError):
            # Do not copy filesystem paths, exception text or command output.
            reason = "operation_recovery_requires_review"
        manager.call(f"/operations/{operation['id']}/reconciliation", {"lease_token": operation["lease_token"], "reason": reason})


def sync_release_catalog(manager, infrastructure):
    catalog = ReleaseCatalog(infrastructure.config, private_write)
    inventory = []
    for directory in sorted(infrastructure.root.iterdir()):
        if not re.fullmatch("[a-f0-9]{32}", directory.name) or not directory.is_dir():
            continue
        if directory.is_symlink():
            raise ProvisioningError("unsafe_instance_inventory")
        if infrastructure.config.get('backend')=='kubernetes':
            import kubernetes_lifecycle
            installed=kubernetes_lifecycle.installed_release_inventory(infrastructure,directory)
            if installed is not None:inventory.append(installed)
            continue
        manifest = directory / "compose.json"
        if not manifest.exists():
            continue
        value = catalog.read(manifest)["services"]["api"]["environment"]
        instance, org = value["COWORK_INSTANCE_ID"], value["COWORK_ORGANISATION_ID"]
        if identifier(instance) != directory.name:
            raise ProvisioningError("instance_inventory_scope_mismatch")
        identifier(org)
        inventory.append({"instance_id": instance, "organisation_id": org, "release": image_set_id(infrastructure.deployed_images(directory))})
    binding = executor_host.manager_binding(infrastructure.config)
    manager.call("/releases/sync", {"releases": catalog.manifests(), "instances": inventory,
                                  **({'executor_host_binding': binding} if binding is not None else {})})


def work(config, once=False):
    manager, infrastructure = Manager(config), Infrastructure(config)
    attestor = host_capacity_attestation.CapacityAttestor(manager, infrastructure)
    manager.capacity_attestor = infrastructure.capacity_attestor = attestor
    disk_renewer=instance_disk_quotas.Renewer(manager,infrastructure)
    try:
        attestor.start()
        disk_renewer.start()
        _work(manager, infrastructure, once)
    finally:
        disk_renewer.close()
        attestor.close()


def _work(manager, infrastructure, once=False):
    config = infrastructure.config
    catalog_synced = 0
    while True:
        vm_snapshots.process(manager,infrastructure)
        environment_lifecycle.process(manager,infrastructure)
        environment_execution.process(manager,infrastructure)
        reconcile_operations(manager, infrastructure)
        if config.get("release_catalog_sync") and time.monotonic() - catalog_synced > 30:
            sync_release_catalog(manager, infrastructure)
            catalog_synced = time.monotonic()
        operation = manager.call("/claim", operator_contract(config))
        if operation:
            journal = OperationJournal(infrastructure, operation)
            complete = journal.recover()
            if complete is None:
                journal.begin()
                try:
                    result = infrastructure.execute(operation)
                    succeeded = True
                except ExecutionGenerationError:
                    manager.call(f"/operations/{operation['id']}/reconciliation", {"lease_token": operation["lease_token"], "reason": "operation_recovery_requires_review"})
                    if once:
                        return
                    time.sleep(2)
                    continue
                except ProvisioningError as error:
                    if operation.get('execution_recovery_binding') is not None or operation["action"] == "delete" or (operation["action"] in ["upgrade", "rollback"] and error.instance_state != "stopped"):
                        # Keep the journal incomplete and retain the original
                        # lease. Only explicit source reconciliation can unlock it.
                        manager.call(f"/operations/{operation['id']}/reconciliation", {"lease_token": operation["lease_token"], "reason": "operation_recovery_requires_review"})
                        if once:
                            return
                        time.sleep(2)
                        continue
                    result, succeeded = {**error.result, "error_code": str(error)}, False
                    if error.instance_state:
                        result["instance_state"] = error.instance_state
                try:
                    complete = journal.finish(succeeded, result)
                except ExecutionGenerationError:
                    manager.call(f"/operations/{operation['id']}/reconciliation", {"lease_token": operation["lease_token"], "reason": "operation_recovery_requires_review"})
                    if once:
                        return
                    time.sleep(2)
                    continue
            deliver_completion(manager, operation, complete)
            catalog_synced = 0
        if once:
            return
        time.sleep(2)


def operator_contract(config):
    binding = executor_host.manager_binding(config)
    return {'backend':config['backend'],**({'execution_admission_version':1} if config.get('execution_admission') is not None else {}),
            **({'backend_profile_version':1} if config.get('backend_profile') is not None else {}),
            **({'execution_enrollment_version':1} if config.get('execution_enrollment_enabled') is True else {}),
            **({'provisioning_release_version':1} if config.get('release_selection_enabled') is True else {}),
            **({'executor_host_binding': binding} if binding is not None else {})}


def main() -> None:
    if os.name != "posix":
        raise ProvisioningError("provisioner_requires_linux_operator_host")
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True)
    parser.add_argument("--once", action="store_true")
    args = parser.parse_args()
    config = json.loads(Path(args.config).read_text())
    if host_capacity_attestation.settings(config) is not None:
        host_capacity_attestation.private_configuration(args.config)
    infrastructure = Infrastructure(config)
    with operator_lock(infrastructure.root):
        work(config, args.once)


if __name__ == "__main__":
    main()
