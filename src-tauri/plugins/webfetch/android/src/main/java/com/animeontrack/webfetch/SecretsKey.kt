package com.animeontrack.webfetch

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

object SecretsKey {
    private const val KEY_ALIAS = "aot_secrets_wrap"
    private const val PREFS_NAME = "aot_secrets"
    private const val PREF_WRAPPED_KEY = "wrapped_data_key"
    private const val ANDROID_KEYSTORE = "AndroidKeyStore"
    private const val AES_GCM_NOPADDING = "AES/GCM/NoPadding"
    private const val GCM_TAG_LENGTH = 128
    private const val IV_LENGTH = 12
    private const val DATA_KEY_SIZE = 32

    @Synchronized
    fun get(context: Context): String {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val wrappedB64 = prefs.getString(PREF_WRAPPED_KEY, null)
        if (wrappedB64 != null) {
            try {
                val dataKey = unwrap(wrappedB64)
                if (dataKey != null && dataKey.size == DATA_KEY_SIZE) {
                    return Base64.encodeToString(dataKey, Base64.NO_WRAP)
                }
            } catch (e: javax.crypto.BadPaddingException) {
                // Includes AEADBadTagException: the blob does not match the Keystore key
                // (corrupted or key replaced): regenerate below.
            } catch (e: java.security.UnrecoverableKeyException) {
                // The Keystore entry exists but cannot be loaded any more: regenerate below.
            } catch (e: android.security.keystore.KeyPermanentlyInvalidatedException) {
                // Keystore key invalidated by the system: regenerate below.
            } catch (e: IllegalArgumentException) {
                // Stored blob is not valid base64: regenerate below.
            }
            // Any other failure (a transient Keystore error) propagates instead of
            // replacing the key, which would make every stored secret unreadable.
        }

        return generateAndStore(prefs)
    }

    private fun unwrap(wrappedB64: String): ByteArray? {
        val blob = Base64.decode(wrappedB64, Base64.NO_WRAP)
        if (blob.size < IV_LENGTH + 16 + DATA_KEY_SIZE) {
            return null
        }
        val iv = blob.copyOfRange(0, IV_LENGTH)
        val ciphertext = blob.copyOfRange(IV_LENGTH, blob.size)

        val keyStore = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        val secretKey = keyStore.getKey(KEY_ALIAS, null) as? SecretKey ?: return null

        val cipher = Cipher.getInstance(AES_GCM_NOPADDING)
        val spec = GCMParameterSpec(GCM_TAG_LENGTH, iv)
        cipher.init(Cipher.DECRYPT_MODE, secretKey, spec)
        return cipher.doFinal(ciphertext)
    }

    private fun generateAndStore(prefs: android.content.SharedPreferences): String {
        val keyStore = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        if (keyStore.containsAlias(KEY_ALIAS)) {
            try {
                keyStore.deleteEntry(KEY_ALIAS)
            } catch (e: Exception) {
                // Ignore failure deleting old entry
            }
        }

        val keyGenerator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        val keyGenSpec = KeyGenParameterSpec.Builder(
            KEY_ALIAS,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .build()
        keyGenerator.init(keyGenSpec)
        val wrapKey = keyGenerator.generateKey()

        val dataKey = ByteArray(DATA_KEY_SIZE)
        SecureRandom().nextBytes(dataKey)

        val cipher = Cipher.getInstance(AES_GCM_NOPADDING)
        cipher.init(Cipher.ENCRYPT_MODE, wrapKey)
        val iv = cipher.iv
        val ciphertext = cipher.doFinal(dataKey)

        val blob = ByteArray(iv.size + ciphertext.size)
        System.arraycopy(iv, 0, blob, 0, iv.size)
        System.arraycopy(ciphertext, 0, blob, iv.size, ciphertext.size)

        val newWrappedB64 = Base64.encodeToString(blob, Base64.NO_WRAP)
        // commit(), not apply(): if the process died before an async write landed, the
        // next start would generate another key and lose every secret sealed with this one.
        check(prefs.edit().putString(PREF_WRAPPED_KEY, newWrappedB64).commit()) { "could not store the wrapped data key" }

        return Base64.encodeToString(dataKey, Base64.NO_WRAP)
    }
}
