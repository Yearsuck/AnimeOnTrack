package com.animeontrack.webfetch

import android.annotation.SuppressLint
import android.app.Activity
import android.graphics.Bitmap
import android.net.Uri
import android.net.http.SslError
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.RenderProcessGoneDetail
import android.webkit.SslErrorHandler
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import org.json.JSONObject
import org.json.JSONTokener
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress

@InvokeArg
class FetchArgs {
    lateinit var url: String
    /** How long to wait for the readiness probe before giving up. */
    var timeoutMs: Long = 40000
    /** How long reading the ready page's HTML may take. */
    var readTimeoutMs: Long = 15000
    /** Optional synchronous script run on the loaded page after the HTML is captured. */
    var extraScript: String? = null
    var extraTimeoutMs: Long = 90000
    /** Add the WebView to the activity as a 1x1 view (renders like a real tab) instead of keeping it detached. */
    var attach: Boolean = true
}

/**
 * Navigation rules for the scraper WebView. Same host rules as `is_safe_external_url`
 * in scraper_engine.rs (keep them in sync): only http/https (plus about/data/blob), no
 * localhost, and no literal loopback/private/link-local/CGNAT/unspecified IPv4, no IPv6
 * loopback/unspecified/unique-local/link-local, and IPv4 embedded in IPv6 (mapped,
 * compatible, NAT64 64:ff9b::/96, 6to4 2002::/16) is checked as IPv4. A hostname that
 * only resolves to such an address is not caught, as on desktop.
 *
 * URLs reaching `shouldOverrideUrlLoading` come from Chromium already canonicalised, so
 * forms like `http://2130706433/` arrive as dotted IPv4.
 */
object NavigationRules {
    fun allowed(raw: String): Boolean {
        val uri = Uri.parse(raw)
        return when ((uri.scheme ?: "").lowercase()) {
            "about", "data", "blob" -> true
            "http", "https" -> isSafeHost(uri.host)
            else -> false
        }
    }

    fun isSafeHost(rawHost: String?): Boolean {
        val host = (rawHost ?: "").lowercase().removePrefix("[").removeSuffix("]")
        if (host.isEmpty() || host == "localhost") return false
        ipv4Literal(host)?.let { return isSafeIpv4(it) }
        if (host.contains(':')) {
            // A numeric literal: InetAddress parses it without any DNS lookup.
            val addr = try { InetAddress.getByName(host) } catch (e: Exception) { return false }
            return when (addr) {
                is Inet4Address -> isSafeIpv4(addr.address) // Java folds ::ffff:a.b.c.d to IPv4
                is Inet6Address -> isSafeIpv6(addr.address)
                else -> false
            }
        }
        return true
    }

    private fun ipv4Literal(host: String): ByteArray? {
        val parts = host.split('.')
        if (parts.size != 4) return null
        val out = ByteArray(4)
        for (i in 0 until 4) {
            val p = parts[i]
            if (p.isEmpty() || p.length > 3 || !p.all { it in '0'..'9' }) return null
            val v = p.toInt()
            if (v > 255) return null
            out[i] = v.toByte()
        }
        return out
    }

    private fun u(b: Byte): Int = b.toInt() and 0xff

    fun isSafeIpv4(b: ByteArray): Boolean {
        val a = u(b[0])
        val c = u(b[1])
        return !(a == 0 || a == 127 || a == 10 ||
            (a == 172 && c in 16..31) ||
            (a == 192 && c == 168) ||
            (a == 169 && c == 254) ||
            (a == 100 && (c and 0xc0) == 0x40))
    }

    private fun isSafeIpv6(b: ByteArray): Boolean {
        val firstTwelveZero = (0 until 12).all { b[it].toInt() == 0 }
        val allZero = firstTwelveZero && (12 until 16).all { b[it].toInt() == 0 }
        val loopback = firstTwelveZero && (12 until 15).all { b[it].toInt() == 0 } && u(b[15]) == 1
        if (allZero || loopback) return false
        if ((u(b[0]) and 0xfe) == 0xfc) return false // fc00::/7
        if (u(b[0]) == 0xfe && (u(b[1]) and 0xc0) == 0x80) return false // fe80::/10
        // IPv4-compatible ::a.b.c.d
        if (firstTwelveZero) return isSafeIpv4(b.copyOfRange(12, 16))
        // NAT64 64:ff9b::/96
        val nat64 = byteArrayOf(0, 0x64, 0xff.toByte(), 0x9b.toByte(), 0, 0, 0, 0, 0, 0, 0, 0)
        if ((0 until 12).all { b[it] == nat64[it] }) return isSafeIpv4(b.copyOfRange(12, 16))
        // 6to4 2002::/16
        if (u(b[0]) == 0x20 && u(b[1]) == 0x02) return isSafeIpv4(b.copyOfRange(2, 6))
        return true
    }
}

/**
 * Android backend of AnimeOnTrack's scraper (`scraper_engine.rs`): loads a URL in an
 * off-screen `android.webkit.WebView`, waits for the same readiness probe the desktop
 * WebView2 path uses, and returns the rendered HTML. Never shows anything: the view is
 * 1x1 (or detached) and is destroyed after every fetch. Rust serialises calls, but this
 * class does not rely on it: each call owns its own WebView.
 *
 * Every call always resolves: exactly one deadline (ready, then HTML read, then the
 * optional script) is armed from the start until `finish`, so a dropped
 * `evaluateJavascript` callback cannot leave the Rust caller waiting.
 */
@TauriPlugin
class WebFetchPlugin(private val activity: Activity) : Plugin(activity) {
    private val main = Handler(Looper.getMainLooper())

    // Same readiness condition as scraper_engine.rs `PROBE` (keep them in sync).
    private val probe = "JSON.stringify({" +
        "ready: (document.readyState==='interactive' || document.readyState==='complete')" +
        " && !!document.body" +
        " && (document.body.innerHTML.length>3000 || (document.contentType==='application/json' && document.body.innerText.trim().length>1))" +
        " && !/just a moment|un momento|checking your browser|verificando|502|503|504|bad gateway|gateway time-out|service unavailable/i.test(document.title)," +
        "readyState: document.readyState, title: document.title," +
        "len: document.body ? document.body.innerHTML.length : -1})"

    @Command
    fun fetchRendered(invoke: Invoke) {
        val args = invoke.parseArgs(FetchArgs::class.java)
        main.post {
            try {
                run(invoke, args)
            } catch (e: Exception) {
                invoke.reject("webfetch failed to start: " + e.message)
            }
        }
    }

    /** `evaluateJavascript` hands back a JSON value; a JS string comes back quoted. Null if not a string. */
    private fun unquote(raw: String?): String? =
        try { JSONTokener(raw ?: "null").nextValue() as? String } catch (e: Exception) { null }

    @SuppressLint("SetJavaScriptEnabled")
    private fun run(invoke: Invoke, args: FetchArgs) {
        val started = System.currentTimeMillis()
        val wv = WebView(activity)
        wv.settings.javaScriptEnabled = true
        wv.settings.domStorageEnabled = true
        // A scraped page never needs local files or content providers.
        wv.settings.allowFileAccess = false
        wv.settings.allowContentAccess = false
        // Never autoplay media from a scraped page.
        wv.settings.mediaPlaybackRequiresUserGesture = true
        val cookies = CookieManager.getInstance()
        cookies.setAcceptCookie(true)
        cookies.setAcceptThirdPartyCookies(wv, true)
        // Default user agent on purpose: a spoofed desktop UA is what bot checks look for.

        var finished = false
        var lastProbe = "{}"
        var polls = 0
        var readyMs = -1L
        // URL of the main-frame load in progress (follows redirects), for matching SSL errors.
        var mainUrl = args.url

        fun finish(ok: Boolean, error: String, html: String, extra: String?) {
            if (finished) return
            finished = true
            // Drops the armed deadline and any pending probe tick (all posted with token `wv`).
            main.removeCallbacksAndMessages(wv)
            val elapsed = System.currentTimeMillis() - started
            val title = try { JSONObject(lastProbe).optString("title") } catch (e: Exception) { "" }
            val finalUrl = wv.url ?: ""
            val res = JSObject()
            res.put("ok", ok)
            res.put("error", error)
            res.put("html", html)
            res.put("hasExtra", extra != null)
            res.put("extra", extra ?: "")
            res.put("title", title)
            res.put("finalUrl", finalUrl)
            res.put("elapsedMs", elapsed)
            res.put("readyMs", readyMs)
            res.put("polls", polls)
            Log.i(
                "AOT-webfetch",
                "url=${args.url} ok=$ok error=$error polls=$polls readyMs=$readyMs elapsedMs=$elapsed " +
                    "htmlLen=${html.length} extra=${extra?.length ?: -1} final=$finalUrl last=$lastProbe"
            )
            // Tear the WebView down completely: no view, no page, no timers left behind.
            try {
                wv.stopLoading()
                (wv.parent as? ViewGroup)?.removeView(wv)
                wv.webViewClient = WebViewClient()
                wv.destroy()
            } catch (e: Exception) {
                Log.w("AOT-webfetch", "destroy failed: " + e.message)
            }
            cookies.flush()
            invoke.resolve(res)
        }

        // The single deadline: re-arming replaces the previous one.
        var deadline: Runnable? = null
        fun arm(ms: Long, error: String) {
            deadline?.let { main.removeCallbacks(it) }
            val r = Runnable { finish(false, error, "", null) }
            deadline = r
            main.postAtTime(r, wv, SystemClock.uptimeMillis() + ms)
        }

        wv.webViewClient = object : WebViewClient() {
            override fun shouldOverrideUrlLoading(v: WebView, r: WebResourceRequest): Boolean {
                val url = r.url.toString()
                if (!NavigationRules.allowed(url)) return true
                if (r.isForMainFrame) mainUrl = url
                return false
            }

            override fun onPageStarted(v: WebView, url: String, favicon: Bitmap?) {
                mainUrl = url
            }

            override fun onReceivedError(v: WebView, r: WebResourceRequest, e: WebResourceError) {
                // A main-frame network failure (DNS, TLS, refused) is final for this load:
                // fail now instead of polling an error page until the timeout.
                // ERR_ABORTED is a navigation replaced by another one (a challenge redirect), not a failure.
                if (r.isForMainFrame && !e.description.toString().contains("ERR_ABORTED")) {
                    finish(false, "main frame error " + e.errorCode + " " + e.description, "", null)
                }
            }

            override fun onReceivedSslError(v: WebView, h: SslErrorHandler, e: SslError) {
                h.cancel()
                // Only the main-frame load is fatal (a subresource cert error is not),
                // including one reached through a redirect.
                if (e.url == mainUrl || e.url == v.url || e.url == args.url) {
                    finish(false, "ssl error " + e.primaryError + " on " + e.url, "", null)
                }
            }

            override fun onRenderProcessGone(v: WebView, d: RenderProcessGoneDetail): Boolean {
                // All WebViews of the app, including Tauri's own UI WebView, normally share
                // one renderer process. Returning true only stops the system from killing
                // the app because of THIS view; if the shared renderer died, the app's UI
                // WebView gets its own onRenderProcessGone and may still be lost.
                finish(false, "renderer process gone (crash=" + d.didCrash() + ")", "", null)
                return true
            }
        }

        if (args.attach) {
            // 1x1 view inside the activity content: renders and runs timers like a
            // foreground tab, but is not visible.
            activity.addContentView(wv, ViewGroup.LayoutParams(1, 1))
        }

        fun runExtra(html: String) {
            val script = args.extraScript
            if (script == null) {
                finish(true, "", html, null)
                return
            }
            arm(args.extraTimeoutMs, "episode script timed out after ${args.extraTimeoutMs} ms")
            wv.evaluateJavascript(script) { raw ->
                if (finished) return@evaluateJavascript
                val extra = unquote(raw)
                if (extra == null) {
                    finish(false, "episode script did not return a string", "", null)
                } else {
                    finish(true, "", html, extra)
                }
            }
        }

        fun grabHtml() {
            arm(args.readTimeoutMs, "html read timed out")
            wv.evaluateJavascript("document.documentElement.outerHTML") { raw ->
                if (finished) return@evaluateJavascript
                val html = unquote(raw)
                if (html.isNullOrEmpty()) {
                    finish(false, "could not read page HTML", "", null)
                } else {
                    runExtra(html)
                }
            }
        }

        // Condition-based wait, same back-off as the desktop poll: 150 ms growing to 1 s.
        // The armed deadline (not this loop) decides when to give up.
        var interval = 150L
        lateinit var tick: Runnable
        tick = Runnable {
            if (finished) return@Runnable
            polls += 1
            wv.evaluateJavascript(probe) { raw ->
                if (finished) return@evaluateJavascript
                val inner = unquote(raw)
                if (inner != null) {
                    lastProbe = inner
                    val ready = try { JSONObject(inner).optBoolean("ready") } catch (e: Exception) { false }
                    if (ready) {
                        readyMs = System.currentTimeMillis() - started
                        grabHtml()
                        return@evaluateJavascript
                    }
                }
                interval = minOf(interval + interval / 2, 1000L)
                main.postAtTime(tick, wv, SystemClock.uptimeMillis() + interval)
            }
        }

        arm(args.timeoutMs, "timeout")
        wv.loadUrl(args.url)
        main.postAtTime(tick, wv, SystemClock.uptimeMillis() + interval)
    }
}
