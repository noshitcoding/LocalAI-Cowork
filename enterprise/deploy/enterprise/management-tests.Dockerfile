# Disposable database acceptance. Build the server image first.
FROM postgres:16.10-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends python3 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=cowork-server:local-review /usr/local/bin/cowork-server /usr/local/bin/cowork-server
COPY scripts/test-enterprise-management.py /tests/test-enterprise-management.py
COPY scripts/test-enterprise-oidc.py scripts/enterprise_sso_acceptance.py scripts/test-enterprise-mfa.py scripts/test-enterprise-invitations.py scripts/test-enterprise-content.py /tests/
USER postgres
ENTRYPOINT ["python3", "/tests/test-enterprise-management.py", "--server", "/usr/local/bin/cowork-server"]
