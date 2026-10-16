package com.whilecode.cliproam

import android.os.Bundle
import android.view.KeyEvent
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    // Give modals and history selection first chance to consume native back.
    onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
      override fun handleOnBackPressed() {
        webView.evaluateJavascript(
          """
          (() => {
            if (!document.querySelector('[aria-modal="true"], [data-history-selecting]')) return false;
            document.dispatchEvent(new KeyboardEvent('keydown', {
              key: 'Escape', code: 'Escape', bubbles: true, cancelable: true
            }));
            return true;
          })()
          """.trimIndent()
        ) { handled ->
          if (handled != "true") {
            isEnabled = false
            try {
              onBackPressedDispatcher.onBackPressed()
            } finally {
              isEnabled = true
            }
          }
        }
      }
    })
  }

  override fun dispatchKeyEvent(event: KeyEvent): Boolean {
    if (event.keyCode == KeyEvent.KEYCODE_ESCAPE) {
      if (event.action == KeyEvent.ACTION_UP) onBackPressedDispatcher.onBackPressed()
      return true
    }
    return super.dispatchKeyEvent(event)
  }
}
