import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

// Release signing: keystore.properties (storeFile, storePassword, keyAlias,
// keyPassword) next to this project, or the same as SF2PLAYER_* environment
// variables in CI. Without either, release builds are signed with the debug
// key so that they still install.
val keystore = Properties().apply {
    val file = rootProject.file("keystore.properties")
    if (file.exists()) file.inputStream().use { load(it) }
    System.getenv("SF2PLAYER_STORE_FILE")?.let {
        setProperty("storeFile", it)
        setProperty("storePassword", System.getenv("SF2PLAYER_STORE_PASSWORD"))
        setProperty("keyAlias", System.getenv("SF2PLAYER_KEY_ALIAS"))
        setProperty("keyPassword", System.getenv("SF2PLAYER_KEY_PASSWORD"))
    }
}

android {
    namespace = "jp.albos.sf2player"
    compileSdk = 36
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "jp.albos.sf2player"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "1.0.0"
        ndk {
            abiFilters += abis
        }
    }

    signingConfigs {
        if (keystore.containsKey("storeFile")) {
            create("release") {
                storeFile = rootProject.file(keystore.getProperty("storeFile"))
                storePassword = keystore.getProperty("storePassword")
                keyAlias = keystore.getProperty("keyAlias")
                keyPassword = keystore.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures {
        compose = true
    }
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.ui)
    implementation(libs.androidx.material3)
    implementation(libs.androidx.ui.tooling.preview)
    debugImplementation(libs.androidx.ui.tooling)
}

// The Rust player (../ffi) as libsf2player_ffi.so for each ABI, built with
// cargo-ndk (`cargo install cargo-ndk`) before every build; cargo only
// rebuilds what changed.
val abis: List<String>
    get() = listOf("arm64-v8a", "armeabi-v7a", "x86_64")

val buildRust by tasks.registering(Exec::class) {
    group = "build"
    description = "Builds the Rust player into src/main/jniLibs."
    // Android Studio doesn't see the shell's PATH: look where rustup puts cargo too.
    val home = System.getProperty("user.home")
    val path = listOf("$home/.cargo/bin", "/opt/homebrew/opt/rustup/bin", "/opt/homebrew/bin", "/usr/local/bin") +
        System.getenv("PATH").orEmpty().split(File.pathSeparator)
    val exe = if (System.getProperty("os.name").startsWith("Windows")) ".exe" else ""
    val cargo = path.map { File(it, "cargo$exe") }.firstOrNull { it.canExecute() }?.path ?: "cargo"
    val ndk = android.ndkDirectory
    val out = file("src/main/jniLibs")
    workingDir = rootDir.resolve("..")
    environment("PATH", path.joinToString(File.pathSeparator))
    doFirst { environment("ANDROID_NDK_HOME", ndk.path) }
    commandLine(
        listOf(cargo, "ndk", "--platform", "26") +
            abis.flatMap { listOf("-t", it) } +
            listOf("-o", out.path, "build", "--release", "--package", "sf2player-ffi"),
    )
}

tasks.named("preBuild") { dependsOn(buildRust) }
