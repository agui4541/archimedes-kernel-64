package me.weishu.kernelsu.ui.util

import android.content.Context

/** App-only display scaling; never changes the system wm density. */
object DisplayScale {
    private const val PREFS = "settings"
    private const val KEY = "display_scale"
    private const val DEFAULT = 1.0f

    val options = listOf(0.8f, 0.9f, 1.0f, 1.1f, 1.2f, 1.3f)

    private var state = androidx.compose.runtime.mutableFloatStateOf(DEFAULT)

    var value: Float
        get() = state.floatValue
        private set(newValue) { state.floatValue = newValue }

    fun initialize(context: Context) {
        value = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getFloat(KEY, DEFAULT)
            .coerceIn(options.first(), options.last())
    }

    fun set(context: Context, scale: Float) {
        val normalized = scale.coerceIn(options.first(), options.last())
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putFloat(KEY, normalized).apply()
        value = normalized
    }

    fun label(scale: Float): String = "${(scale * 100).toInt()}%"
}