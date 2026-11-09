FROM golang:1.26.5-alpine AS mc
RUN apk add --no-cache git
RUN CGO_ENABLED=0 go install github.com/minio/mc@77f82e18b5401a65958f1619df6ebb994634bd88
FROM docker:28-cli
ENV DOCKER_CONFIG=/tmp/cowork-docker
RUN apk add --no-cache python3 py3-yaml postgresql16-client age helm kubectl
COPY --from=mc /go/bin/mc /usr/local/bin/mc
WORKDIR /opt/cowork-provisioner
COPY server/cowork-provisioner/provisioner.py server/cowork-provisioner/instance_backups.py server/cowork-provisioner/operation_journal.py server/cowork-provisioner/fleet_images.py server/cowork-provisioner/local_entrypoint.py ./
COPY server/cowork-provisioner/release_catalog.py server/cowork-provisioner/instance_rollbacks.py server/cowork-provisioner/instance_updates.py ./
COPY server/cowork-provisioner/publish_release.py ./
COPY server/cowork-provisioner/instance_deletion.py ./
COPY server/cowork-provisioner/execution_generation.py ./
COPY server/cowork-provisioner/execution_runtime.py ./
COPY server/cowork-provisioner/organisation_mfa.py ./
COPY server/cowork-provisioner/execution_recovery.py ./
COPY server/cowork-provisioner/execution_enrollment.py ./
COPY server/cowork-provisioner/instance_enrollment.py ./
COPY server/cowork-provisioner/provisioning_release.py ./
COPY server/cowork-provisioner/resource_budget.py server/cowork-provisioner/executor_host.py ./
COPY server/cowork-provisioner/remote_entrypoint.py ./
COPY server/cowork-provisioner/guest_endpoints.py ./
COPY server/cowork-provisioner/host_capacity_attestation.py ./
COPY server/cowork-provisioner/remote_volume_streaming.py ./
COPY server/cowork-provisioner/remote_lifecycle_qualification.py ./
COPY server/cowork-provisioner/instance_imports.py ./
COPY server/cowork-provisioner/instance_import_target.py ./
COPY server/cowork-provisioner/instance_gateway.py ./
COPY server/cowork-provisioner/backend_profile.py server/cowork-provisioner/kubernetes_lifecycle.py server/cowork-provisioner/kubernetes_archive.py server/cowork-provisioner/kubernetes_capacity.py ./
COPY server/cowork-provisioner/environment_lifecycle.py ./
COPY server/cowork-provisioner/environment_execution.py server/cowork-provisioner/vm_snapshots.py ./
COPY server/cowork-provisioner/environment_file_operation.py ./
COPY server/cowork-provisioner/external_vm.py ./
COPY server/cowork-provisioner/external_vm_power.py ./
COPY server/cowork-provisioner/openshell_environment.py ./
COPY server/cowork-provisioner/instance_disk_quotas.py server/cowork-provisioner/disk_quota_guest.py ./
ENTRYPOINT ["python3", "local_entrypoint.py"]
