"""Explicit Docker Desktop operator profile; never mounted into a workload."""
import json
import os
import sys
from pathlib import Path
import provisioner as p
from fleet_images import pinned_fleet_images, retain_fleet_images
MANAGEMENT_URL = 'https://operator-control:8443'



def resolve_images():
    images = {}
    for key, image in {"server": "cowork-server", "runner": "cowork-runner", "sandbox_core": "cowork-sandbox-core",
                       "sandbox_gui": "cowork-sandbox-gui", "sandbox_crew": "cowork-sandbox-crew", "egress_proxy": "cowork-egress"}.items():
        digests = json.loads(p.run(["docker", "image", "inspect", image + ":local-review", "--format", "{{json .RepoDigests}}"] ))
        images[key] = p.immutable_image(next(digest for digest in digests if digest.startswith(image + "@")))
    return images


def execution_configuration(config):
    release_selection = os.environ.get("COWORK_RELEASE_SELECTION_ENABLED", "false")
    if release_selection not in ["true", "false"]:
        raise p.ProvisioningError("invalid_release_selection_configuration")
    if release_selection == "true":
        config["release_selection_enabled"] = True
    admission_file = os.environ.get('COWORK_EXECUTION_ADMISSION_CONFIG_FILE')
    if admission_file:
        # An operator-owned allowlist; never accepted from management requests.
        config['execution_admission'] = p.execution_generation.document(Path(admission_file), 32768)
    enrollment = os.environ.get('COWORK_EXECUTION_ENROLLMENT_ENABLED', 'false')
    if enrollment not in ['true','false']:
        raise p.ProvisioningError('invalid_execution_enrollment_configuration')
    if enrollment == 'true':
        if 'execution_admission' not in config:
            raise p.ProvisioningError('execution_enrollment_requires_admission_allowlist')
        config['execution_enrollment_enabled'] = True


def resource_configuration(config):
    keys = {'cpu_millis': 'COWORK_FLEET_HOST_CPU_MILLIS', 'memory_mib': 'COWORK_FLEET_HOST_MEMORY_MIB',
            'reserved_cpu_millis': 'COWORK_FLEET_RESERVED_CPU_MILLIS', 'reserved_memory_mib': 'COWORK_FLEET_RESERVED_MEMORY_MIB'}
    values = {key: os.environ.get(env, '') for key, env in keys.items()}
    if any(values.values()):
        try:
            if not all(values.values()):
                raise ValueError()
            config['fleet_host_budget'] = {key: int(value) for key, value in values.items()}
            p.resource_budget.host_budget(config)
        except (ValueError, p.resource_budget.BudgetError):
            raise p.ProvisioningError('invalid_fleet_host_budget_configuration') from None


def main():
    if os.environ.get('COWORK_EXECUTOR_HOST_CONFIG_FILE'):
        raise p.ProvisioningError('remote_operator_requires_remote_entrypoint')
    if any(os.environ.get(key) for key in ['DOCKER_HOST', 'DOCKER_CONTEXT', 'DOCKER_TLS_VERIFY', 'DOCKER_CERT_PATH']):
        raise p.ProvisioningError('local_operator_remote_daemon_override_forbidden')
    state = Path(os.environ["COWORK_PROVISIONER_STATE"])
    volume = os.environ["COWORK_PROVISIONER_VOLUME"]
    actual = p.run(["docker", "volume", "inspect", volume, "--format", "{{.Mountpoint}}"] ).decode().strip()
    if actual != str(state):
        raise p.ProvisioningError("operator_volume_must_match_daemon_mountpoint")
    state.mkdir(parents=True, exist_ok=True)
    state.chmod(0o700)
    with p.operator_lock(state / "instances"):
        mc_dir = state / "mc"
        mc_dir.mkdir(mode=0o700, exist_ok=True)
        p.run(["mc", "--config-dir", str(mc_dir), "alias", "set", "fleet", "http://fleet-storage:9000",
               Path("/run/secrets/fleet_s3_user").read_text().strip(), Path("/run/secrets/fleet_s3_password").read_text().strip()])
        images = pinned_fleet_images(state, resolve_images, p.immutable_image, p.private_write)
        retain_fleet_images(images, p.run, p.immutable_image)
        config = {"backend": "compose", "management_url": MANAGEMENT_URL, "fleet_management_url": "http://fleet-management:8080",
                  "management_ca_file": "/run/secrets/control_tls_certificate",
                  "provisioner_token_file": "/run/secrets/provisioner_token", "state_directory": str(state / "instances"),
                  "backup_directory": str(state / "backups"), "backup_identity_file": "/run/secrets/fleet_backup_identity",
                  "release_directory": str(state / "releases"),
                  "release_catalog_sync": os.environ.get("COWORK_RELEASE_CATALOG_SYNC", "false") == "true",
                  "deletion_enabled": os.environ.get("COWORK_INSTANCE_DELETION_ENABLED", "false") == "true",
                  "database_endpoint": "fleet-db:5432", "database_admin_url_file": "/run/secrets/fleet_database_url",
                  "s3_endpoint": "http://fleet-storage:9000", "s3_admin_alias": "fleet", "s3_admin_config_directory": str(mc_dir),
                  "services_network": "cowork-fleet-services", "routing_network": "cowork-fleet-routing", "images": images,
                  "compose_network_pool": os.environ.get("COWORK_FLEET_NETWORK_POOL", "10.240.0.0/16"),
                  "sandbox_security_profile": "docker_desktop_seccomp", "desktop_origins": "http://127.0.0.1:5173"}
        execution_configuration(config)
        resource_configuration(config)
        p.verify_security_host(config)
        config_file = state / "config.json"
        p.private_write(config_file, json.dumps(config).encode())
        p.work(config)


if __name__ == "__main__":
    if sys.argv[1:]==['--network-contract']:
        print(json.dumps({'version':1,'independent_namespace':True,'management_url':MANAGEMENT_URL}))
    else:
        main()
