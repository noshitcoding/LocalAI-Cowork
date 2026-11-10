# Runtime secrets

Generate these files before the first `docker compose up` and restrict them to
the deployment administrator:

```powershell
./deploy/init-secrets.ps1
```

or on Linux:

```sh
sh ./deploy/init-secrets.sh
```

Both commands refuse to overwrite an existing deployment. The `--force`/`-Force`
option is only for an intentional rotation while the complete stack is stopped.

- `bootstrap_token.txt`: a random value of at least 32 characters
- `postgres_password.txt`: a random PostgreSQL password
- `database_url.txt`: `postgres://cowork:PASSWORD@postgres:5432/cowork`
- `minio_root_user.txt`: a non-default MinIO administrative user
- `minio_root_password.txt`: a random value of at least 32 characters
- `runner_signing_key.txt`: a separate random value of at least 32 characters
- `storage_master_key.txt`: a base64-encoded, exactly 32-byte random key; losing
  it makes encrypted snapshots unrecoverable

The bootstrap bearer token is deliberately a first vertical-slice mechanism.
It must be replaced by the full user/session authentication service before an
Internet-facing production deployment is considered complete.
