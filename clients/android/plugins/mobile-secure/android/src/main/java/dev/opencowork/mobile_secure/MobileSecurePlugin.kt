package dev.opencowork.mobile_secure

import android.app.Activity
import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.nio.charset.StandardCharsets
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

private const val ANDROID_KEYSTORE = "AndroidKeyStore"
private const val KEY_ALIAS = "dev.opencowork.android.secure_store.v1"
private const val PREFERENCES = "open_cowork_secure_store_v1"
private const val MAX_NAMESPACE_BYTES = 128
private const val MAX_KEY_BYTES = 512
private const val MAX_VALUE_BYTES = 1024 * 1024

@InvokeArg
class SecretLocator {
    lateinit var namespace: String
    lateinit var key: String
}

@InvokeArg
class StoreRequest {
    lateinit var namespace: String
    lateinit var key: String
    lateinit var value: String
}

@TauriPlugin
class MobileSecurePlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun store(invoke: Invoke) {
        try {
            val request = invoke.parseArgs(StoreRequest::class.java)
            validate(request.namespace, request.key)
            val plaintext = request.value.toByteArray(StandardCharsets.UTF_8)
            require(plaintext.size <= MAX_VALUE_BYTES) { "secure value exceeds 1 MiB" }

            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, getOrCreateKey())
            cipher.updateAAD(aad(request.namespace, request.key))
            val ciphertext = cipher.doFinal(plaintext)
            val encoded = Base64.encodeToString(cipher.iv + ciphertext, Base64.NO_WRAP)
            preferences().edit().putString(storageKey(request.namespace, request.key), encoded).commit()
            invoke.resolve()
        } catch (error: Exception) {
            invoke.reject("secure storage write failed: ${error.message}")
        }
    }

    @Command
    fun retrieve(invoke: Invoke) {
        try {
            val request = invoke.parseArgs(SecretLocator::class.java)
            validate(request.namespace, request.key)
            val encoded = preferences().getString(storageKey(request.namespace, request.key), null)
            val response = JSObject()
            if (encoded == null) {
                response.put("value", null)
                invoke.resolve(response)
                return
            }
            val packed = Base64.decode(encoded, Base64.NO_WRAP)
            require(packed.size > 12 + 16) { "secure value is truncated" }
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(
                Cipher.DECRYPT_MODE,
                getOrCreateKey(),
                GCMParameterSpec(128, packed.copyOfRange(0, 12))
            )
            cipher.updateAAD(aad(request.namespace, request.key))
            val plaintext = cipher.doFinal(packed.copyOfRange(12, packed.size))
            response.put("value", String(plaintext, StandardCharsets.UTF_8))
            invoke.resolve(response)
        } catch (error: Exception) {
            invoke.reject("secure storage read failed: ${error.message}")
        }
    }

    @Command
    fun remove(invoke: Invoke) {
        try {
            val request = invoke.parseArgs(SecretLocator::class.java)
            validate(request.namespace, request.key)
            preferences().edit().remove(storageKey(request.namespace, request.key)).commit()
            invoke.resolve()
        } catch (error: Exception) {
            invoke.reject("secure storage delete failed: ${error.message}")
        }
    }

    private fun getOrCreateKey(): SecretKey {
        val keyStore = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        (keyStore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        val specification = KeyGenParameterSpec.Builder(
            KEY_ALIAS,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .setRandomizedEncryptionRequired(true)
            .setUnlockedDeviceRequired(true)
            .build()
        generator.init(specification)
        return generator.generateKey()
    }

    private fun preferences() = activity.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

    private fun validate(namespace: String, key: String) {
        require(namespace.isNotBlank() && namespace.toByteArray().size <= MAX_NAMESPACE_BYTES) {
            "invalid secure namespace"
        }
        require(key.isNotBlank() && key.toByteArray().size <= MAX_KEY_BYTES) {
            "invalid secure key"
        }
    }

    private fun storageKey(namespace: String, key: String): String {
        val digest = MessageDigest.getInstance("SHA-256")
            .digest("$namespace\u0000$key".toByteArray(StandardCharsets.UTF_8))
        return digest.joinToString("") { "%02x".format(it) }
    }

    private fun aad(namespace: String, key: String): ByteArray =
        "open-cowork-mobile-secure-v1\u0000$namespace\u0000$key".toByteArray(StandardCharsets.UTF_8)
}
