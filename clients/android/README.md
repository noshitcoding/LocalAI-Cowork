# Open Cowork Android shell

The Android client is a separate thin Tauri 2 shell around the shared React UI.
It controls Runs on one configured server or an already registered personal
device; it never starts an agent runtime or accesses laptop projects directly.

## Supported behavior

- Authorization Code + S256 PKCE login against one canonical server origin
- AndroidKeyStore-backed refresh token, cache key and encrypted offline cache
- offline Run/thread view plus an idempotent Outbox for replies/actions
- explicit file and photo uploads through the Android picker
- Run events, approvals, questions, artifacts and terminal views
- browser/Linux/Windows desktop stream viewing and authenticated takeover
- privacy-safe FCM notifications without prompt, message or filename content
- biometric/PIN application lock where the device supports it

Web and Android never receive stored provider keys in plaintext. They cannot
browse a private desktop project's files outside an explicit upload or Run
snapshot.

## Prerequisites

Match the CI toolchain unless intentionally updating it:

- Node.js 22 and npm
- Rust 1.95.0 with `aarch64-linux-android`, `armv7-linux-androideabi` and
  `x86_64-linux-android` targets
- JDK 17
- Android SDK platform 35 and Build Tools 35.0.0
- Android NDK 27.2.12479018
- Tauri 2 mobile prerequisites and a configured emulator or physical device

The shared UI dependencies live under `app/`; the shell has its own locked CLI
dependency under `clients/android/`.

## Local development

From the repository root:

```powershell
npm ci --prefix app
npm ci --prefix clients/android
cd clients/android
npm run android:init
npm run android:dev
```

`src-tauri/gen/android` is generated platform build output. Do not hand-edit or
commit it as source.

To build an unsigned arm64 acceptance APK:

```powershell
cd clients/android
npm run android:init -- --ci
npm run android:build -- --apk --target aarch64
```

An unsigned artifact verifies compilation only. Do not distribute or treat it
as a production release.

## Firebase client configuration

Copy the public values from `app/.env.android.example` into the build
environment:

```dotenv
VITE_COWORK_ANDROID=true
VITE_COWORK_FIREBASE_PROJECT_ID=example-project
VITE_COWORK_FIREBASE_APPLICATION_ID=1:1234567890:android:example
VITE_COWORK_FIREBASE_API_KEY=public-android-app-key
VITE_COWORK_FIREBASE_SENDER_ID=1234567890
```

These identify the public Firebase Android application. The Firebase
service-account JSON/private key belongs only on the server through the FCM
Compose overlay and must never be placed in an APK, repository secret exposed
to the frontend, local cache or push payload.

Without FCM, the app can still refresh Runs after reconnecting; background push
delivery is unavailable.

## Signed internal APK

`.github/workflows/android.yml` builds and verifies a signed arm64 APK. Configure
these GitHub repository secrets:

- `OPEN_COWORK_ANDROID_KEYSTORE_BASE64`
- `OPEN_COWORK_ANDROID_KEY_ALIAS`
- `OPEN_COWORK_ANDROID_KEY_PASSWORD`
- `OPEN_COWORK_FIREBASE_PROJECT_ID`
- `OPEN_COWORK_FIREBASE_APPLICATION_ID`
- `OPEN_COWORK_FIREBASE_API_KEY`
- `OPEN_COWORK_FIREBASE_SENDER_ID`

The workflow initializes Android, builds the release APK, signs it with
`apksigner`, verifies the certificate, writes `SHA256SUMS` and uploads a
30-day internal artifact. Keep the original keystore and passwords in a tested,
offline recovery location; losing the signing identity breaks normal upgrade
continuity.

For credential-free CI acceptance, manually dispatch the main CI workflow. Its
`android-unsigned-apk` job validates the package ID and SHA-256 but deliberately
does not produce a trusted release.

## Connect to a server

1. Deploy the server on one canonical HTTPS origin and validate Web login,
   WebSocket and SSE through the real reverse proxy.
2. Install the signed APK from the controlled internal artifact channel.
3. Enter the canonical origin. A client installation supports exactly one
   server account; clear/re-enrol the app to change deployments.
4. Complete the external browser PKCE login and return through the registered
   deep link.
5. Enable biometric/PIN lock and notification permission as appropriate.
6. Create a disposable Run, disconnect the network, queue a reply in the Outbox,
   reconnect and verify one idempotent submission.
7. Test an explicit attachment upload and a GUI takeover with reauthentication.

Do not use alternate subdomains as independent API origins. Redirect them to the
canonical origin so cookies, PKCE redirects, passkeys and WebSocket origin
validation remain consistent.

## Offline and security boundaries

- Offline mode is a cache and Outbox, not an on-phone executor.
- Cached records are encrypted with a key protected by AndroidKeyStore.
- Logout/device revocation invalidates the server session; local sensitive cache
  is cleared according to the app lifecycle.
- FCM contains only an opaque wake-up/reference signal.
- Private project files remain on their desktop unless explicitly uploaded.
- Manual GUI takeover requires current Run visibility and reauthentication and
  is audited by the server.
- Rooted or compromised devices are outside the KeyStore trust assumption; use
  mobile-device management for higher-assurance deployments.

## Acceptance checklist

- release APK signature and package ID verified on a clean physical device
- upgrade from the previous signed APK preserves permitted cache/session state
- PKCE login, refresh rotation, logout and server-side device revocation
- biometric/PIN lock and encrypted cache-at-rest inspection
- airplane-mode view, Outbox retry and duplicate suppression
- attachment/photo upload and large-upload failure recovery
- FCM receipt with no user content in notification/service logs
- terminal, browser and desktop stream rendering on phone and tablet sizes
- GUI takeover reauthentication, input, clipboard policy and audit events
- server certificate renewal and reverse-proxy WebSocket/SSE behavior

Record physical-device results in the distributed implementation-status matrix
before promoting a build beyond a trusted pilot.
