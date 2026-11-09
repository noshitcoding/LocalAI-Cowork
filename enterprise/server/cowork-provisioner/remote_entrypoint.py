"""Dedicated host operator. Configuration and secrets are supplied by the operator.

This does not discover infrastructure, enroll legacy state, or qualify blocked
remote lifecycle paths. Use a separate operator state root for each daemon.
"""
import argparse
import os
from pathlib import Path

import executor_host
import provisioner as p


def private_file(path, limit):
    path = Path(path)
    if (not path.is_absolute() or not path.is_file() or path.is_symlink()
            or any(parent.is_symlink() for parent in path.parents)):
        raise p.ProvisioningError('unsafe_remote_operator_file')
    stat = path.stat()
    if (stat.st_nlink != 1 or stat.st_size > limit or stat.st_uid != os.geteuid()
            or stat.st_mode & 0o077):
        raise p.ProvisioningError('unsafe_remote_operator_file')
    return path


def configuration(path):
    if os.name != 'posix':
        raise p.ProvisioningError('remote_operator_requires_linux')
    config = p.execution_generation.document(private_file(path, 65536), 65536)
    if executor_host.settings(config) is None:
        raise p.ProvisioningError('remote_operator_executor_host_required')
    private_file(config['provisioner_token_file'], 4096)
    # Existing Manager validation requires HTTPS and forbids endpoint credentials.
    # Docker mTLS and physical capacity are verified by ExecutorHost before use.
    p.resource_budget.host_budget(config)
    p.host_capacity_attestation.settings(config)
    p.remote_lifecycle_qualification.settings(config)
    return config


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--config', required=True)
    parser.add_argument('--once', action='store_true')
    args = parser.parse_args()
    config = configuration(args.config)
    infrastructure = p.Infrastructure(config)
    with p.operator_lock(infrastructure.root):
        infrastructure.host.bind()
        p.work(config, args.once)


if __name__ == '__main__':
    main()
