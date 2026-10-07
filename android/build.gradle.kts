// Top-level build file. Plugin versions are declared here and applied per-module.
plugins {
    // AGP 9 compiles Kotlin itself (built-in Kotlin), so the separate
    // org.jetbrains.kotlin.android plugin is no longer applied.
    id("com.android.application") version "9.4.1" apply false
}
