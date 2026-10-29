package com.leehow.tifmrcview

import android.app.Activity
import android.content.ContentUris
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.provider.Settings
import android.webkit.WebView
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File

/**
 * Files for the Rust side (src/android.rs). The readers want real paths: they
 * map files and list sibling files for the thumbnail strip, so the app asks
 * for all-files access and turns content:// links back into paths, copying
 * the file into the cache only when a provider hides where it lives.
 */
@TauriPlugin
class TmvPlugin(private val activity: Activity) : Plugin(activity) {
  private var opened: String? = null
  private var webView: WebView? = null

  override fun load(webView: WebView) {
    this.webView = webView
    askAllFilesAccess()
    take(activity.intent)
  }

  override fun onNewIntent(intent: Intent) {
    // The page fetches it through take_pending_open, as for desktop opens.
    if (take(intent)) webView?.evaluateJavascript("window.__TAURI__.event.emit('open-pending')", null)
  }

  private fun take(intent: Intent?): Boolean {
    val uri: Uri = when (intent?.action) {
      Intent.ACTION_VIEW -> intent.data
      Intent.ACTION_SEND -> @Suppress("DEPRECATION") (intent.getParcelableExtra(Intent.EXTRA_STREAM) as? Uri)
      else -> null
    } ?: return false
    // tifmrcview:// install links belong to the deep-link plugin.
    if (uri.scheme != "content" && uri.scheme != "file") return false
    opened = resolve(uri) ?: return false
    return true
  }

  private fun askAllFilesAccess() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R || Environment.isExternalStorageManager()) return
    try {
      activity.startActivity(
        Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:${activity.packageName}"))
      )
    } catch (e: Exception) {
      activity.startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
    }
  }

  private fun reply(invoke: Invoke, path: String?) {
    val r = JSObject()
    if (path != null) r.put("path", path)
    invoke.resolve(r)
  }

  @Command
  fun takeOpened(invoke: Invoke) {
    reply(invoke, opened)
    opened = null
  }

  @Command
  fun pickFile(invoke: Invoke) {
    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*")
    startActivityForResult(invoke, intent, "picked")
  }

  @ActivityCallback
  fun picked(invoke: Invoke, result: ActivityResult) {
    val uri = result.data?.data
    reply(invoke, if (result.resultCode == Activity.RESULT_OK && uri != null) resolve(uri) else null)
  }

  private fun resolve(uri: Uri): String? {
    if (uri.scheme == "file") return uri.path
    try {
      if (DocumentsContract.isDocumentUri(activity, uri)) {
        val id = DocumentsContract.getDocumentId(uri)
        when (uri.authority) {
          "com.android.externalstorage.documents" -> {
            val parts = id.split(":", limit = 2)
            val root = if (parts[0].equals("primary", true)) Environment.getExternalStorageDirectory().path
              else "/storage/${parts[0]}"
            val f = File(root, parts.getOrElse(1) { "" })
            if (f.canRead()) return f.path
          }
          "com.android.providers.downloads.documents" -> {
            if (id.startsWith("raw:")) return id.removePrefix("raw:")
            if (id.startsWith("msf:")) {
              val media = ContentUris.withAppendedId(MediaStore.Files.getContentUri("external"), id.removePrefix("msf:").toLong())
              dataColumn(media)?.let { return it }
            }
          }
          "com.android.providers.media.documents" -> {
            val parts = id.split(":", limit = 2)
            if (parts.size == 2) {
              val media = ContentUris.withAppendedId(MediaStore.Files.getContentUri("external"), parts[1].toLong())
              dataColumn(media)?.let { return it }
            }
          }
        }
      }
      dataColumn(uri)?.let { return it }
    } catch (e: Exception) {
    }
    return copyToCache(uri)
  }

  /** The `_data` column, which MediaStore still fills for apps with all-files access. */
  private fun dataColumn(uri: Uri): String? = try {
    activity.contentResolver.query(uri, arrayOf(MediaStore.MediaColumns.DATA), null, null, null)?.use { c ->
      if (c.moveToFirst()) c.getString(0)?.takeIf { File(it).canRead() } else null
    }
  } catch (e: Exception) {
    null
  }

  private fun copyToCache(uri: Uri): String? = try {
    var name = "image"
    activity.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
      if (c.moveToFirst()) c.getString(0)?.let { name = File(it).name }
    }
    val dir = File(activity.cacheDir, "opened")
    dir.deleteRecursively()
    dir.mkdirs()
    val out = File(dir, name)
    activity.contentResolver.openInputStream(uri)?.use { input -> out.outputStream().use { input.copyTo(it) } }
    out.path
  } catch (e: Exception) {
    null
  }
}
