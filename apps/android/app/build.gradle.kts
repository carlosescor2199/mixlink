plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.mixlink.android"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.mixlink.android"
        minSdk = 29
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"

    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

dependencies {
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    testImplementation("junit:junit:4.13.2")
    // The mockable android.jar stubs org.json in local unit tests; this real implementation puts it
    // back so JSON round trips exercise genuine parsing instead of returning default values.
    testImplementation("org.json:json:20240303")
}
