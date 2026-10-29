package com.leehow.tifmrcview

import android.os.Bundle
import android.view.View
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Android 15+ draws apps under the status and navigation bars; keep the
    // page clear of them (and of the camera cutout) so the toolbar can be tapped.
    ViewCompat.setOnApplyWindowInsetsListener(findViewById<View>(android.R.id.content)) { v, insets ->
      val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
      v.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
  }
}
