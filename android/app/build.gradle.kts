plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.s380relay.client"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.s380relay.client"
        minSdk = 21            // HCE prefix AIDs need API 21+
        targetSdk = 34
        versionCode = 1
        versionName = "0.1"
    }

    signingConfigs {
        // CI points this at a fixed keystore (see .github/workflows/android.yml)
        // so every build has the same signature; locally the default
        // ~/.android/debug.keystore is used.
        getByName("debug") {
            System.getenv("DEBUG_KEYSTORE")?.let { storeFile = file(it) }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.appcompat:appcompat:1.7.0")
}
