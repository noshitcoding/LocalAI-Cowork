# Open Cowork Android shell

This is a separate thin Tauri 2 Android shell. It consumes the shared React UI
from `app/` and never starts an agent runtime on the phone. Runs are created on
the configured server or routed to an already registered personal device.

Prerequisites are the normal Tauri Android toolchain (Android Studio/SDK, NDK,
Java and Rust Android targets). Then:

```powershell
cd clients/android
npm install
npm run android:init
npm run android:dev
```

The generated Gradle project is platform build output. The shell now uses an
S256 authorization-code/PKCE exchange, keeps refresh tokens and cache keys in
AndroidKeyStore, encrypts its offline cache, supports file/photo uploads and
remote GUI control, and registers privacy-safe FCM notifications. It never
contains a bootstrap token, provider API key, Firebase service-account key, or
agent runtime.

FCM requires the four public Android-app values shown in
`app/.env.android.example`. The Firebase service-account JSON is configured
only on the server through `COWORK_FCM_SERVICE_ACCOUNT_FILE`.

The `android-internal-apk` workflow builds and verifies a signed arm64 APK. It
requires `OPEN_COWORK_ANDROID_KEYSTORE_BASE64`,
`OPEN_COWORK_ANDROID_KEY_ALIAS`, `OPEN_COWORK_ANDROID_KEY_PASSWORD`, and the
four `OPEN_COWORK_FIREBASE_*` repository secrets. No unsigned release artifact
is published.
