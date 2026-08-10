import { createHash, randomBytes } from "node:crypto";

const apiBase = required("COWORK_STORAGE_PRESSURE_API").replace(/\/$/, "");
const token = required("COWORK_STORAGE_PRESSURE_TOKEN");
const projectId = required("COWORK_STORAGE_PRESSURE_PROJECT_ID");
const durationSeconds = positiveInteger("COWORK_STORAGE_PRESSURE_SECONDS", 90);
const concurrency = positiveInteger("COWORK_STORAGE_PRESSURE_CONCURRENCY", 8);
const chunkBytes = positiveInteger("COWORK_STORAGE_PRESSURE_CHUNK_BYTES", 256 * 1024);
const deadline = Date.now() + durationSeconds * 1000;
const sharedBytes = randomBytes(chunkBytes);
const sharedDigest = digest(sharedBytes);
const latencies = [];
let completed = 0;
let sharedCompleted = 0;
let uniqueCompleted = 0;
let uploaded = 0;
let deduplicated = 0;

function required(name) {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`Missing ${name}`);
  return value;
}

function positiveInteger(name, fallback) {
  const value = Number.parseInt(process.env[name] ?? `${fallback}`, 10);
  if (!Number.isSafeInteger(value) || value <= 0) throw new Error(`${name} must be a positive integer`);
  return value;
}

function digest(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

async function request(path, options = {}) {
  const response = await fetch(`${apiBase}${path}`, {
    ...options,
    signal: AbortSignal.timeout(60_000),
    headers: {
      authorization: `Bearer ${token}`,
      ...(options.body && !(options.body instanceof Uint8Array) ? { "content-type": "application/json" } : {}),
      ...options.headers,
    },
  });
  if (!response.ok) {
    const detail = (await response.text()).slice(0, 2000);
    throw new Error(`${options.method ?? "GET"} ${path} returned ${response.status}: ${detail}`);
  }
  return response;
}

async function json(path, method, body) {
  const response = await request(path, {
    method,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (response.status === 204) return null;
  return response.json();
}

async function exercise(worker, iteration) {
  const useShared = (worker + iteration) % 2 === 0;
  const bytes = useShared ? sharedBytes : randomBytes(chunkBytes);
  const chunkDigest = useShared ? sharedDigest : digest(bytes);
  const started = performance.now();
  const upload = await json("/snapshots", "POST", {
    project_id: projectId,
    total_bytes: bytes.length,
    files: [{
      path: `pressure/${worker}/${iteration}-${useShared ? "shared" : "unique"}.bin`,
      size: bytes.length,
      mode: 420,
      modified_at: new Date().toISOString(),
      chunks: [{ digest: chunkDigest, plaintext_size: bytes.length }],
    }],
    expires_at: new Date(Date.now() + 24 * 60 * 60 * 1000).toISOString(),
  });

  if (upload.missing_chunks.includes(chunkDigest)) {
    const receipt = await request(`/snapshots/${upload.manifest_id}/chunks/${chunkDigest}`, {
      method: "PUT",
      headers: { "content-type": "application/octet-stream" },
      body: bytes,
    }).then((response) => response.json());
    if (receipt.deduplicated) deduplicated += 1;
    else uploaded += 1;
  } else {
    deduplicated += 1;
  }

  const manifest = await json(`/snapshots/${upload.manifest_id}/commit`, "POST", {});
  const downloaded = new Uint8Array(await request(
    `/snapshots/${manifest.id}/chunks/${chunkDigest}`,
  ).then((response) => response.arrayBuffer()));
  if (digest(downloaded) !== chunkDigest) {
    throw new Error(`Worker ${worker} iteration ${iteration} failed encrypted integrity roundtrip`);
  }
  await json(`/snapshots/${manifest.id}`, "DELETE");
  latencies.push(performance.now() - started);
  completed += 1;
  if (useShared) sharedCompleted += 1;
  else uniqueCompleted += 1;
}

async function workerLoop(worker) {
  let iteration = 0;
  do {
    await exercise(worker, iteration);
    iteration += 1;
  } while (Date.now() < deadline);
}

await Promise.all(Array.from({ length: concurrency }, (_, index) => workerLoop(index)));
latencies.sort((left, right) => left - right);
const percentile = (value) => latencies[Math.min(latencies.length - 1, Math.floor(latencies.length * value))];
const summary = {
  durationSeconds,
  concurrency,
  chunkBytes,
  completed,
  sharedCompleted,
  uniqueCompleted,
  uploaded,
  deduplicated,
  p50Ms: Math.round(percentile(0.5)),
  p95Ms: Math.round(percentile(0.95)),
  maxMs: Math.round(latencies.at(-1)),
};
if (completed < concurrency * 2 || sharedCompleted === 0 || uniqueCompleted === 0 || uploaded === 0 || deduplicated === 0) {
  throw new Error(`Storage pressure did not exercise every required path: ${JSON.stringify(summary)}`);
}
console.log(JSON.stringify(summary));
console.log("parallel_encrypted_roundtrips=ok");
console.log("concurrent_scope_deduplication=ok");
console.log("delete_gc_pressure=ok");
