import java.io.File

plugins {
    alias(libs.plugins.android.application)
    // Kotlin 支持由 AGP 9 内建(built-in Kotlin:Kotlin 编译器随 AGP 走,
    // org.jetbrains.kotlin.android 在 AGP 9 被直接拒绝);Compose 编译器
    // 插件仍需单独引入(buildFeatures.compose 开启时强制要求)。
    alias(libs.plugins.kotlin.compose)
}

// ── MTDSS 引擎(mtdssd)交叉编译与打包 ─────────────────────────────────────
// 布局:本 Gradle 工程位于仓库根的 android/(与 cargo workspace 平级,设计 §7),
// 仓库根即本目录的上级。
val repoRoot: File = rootProject.projectDir.parentFile

// cargo 可执行文件:优先 $CARGO_HOME/bin(Windows 本机 PATH 常不含 cargo),
// 否则裸命令交 PATH 解析。
val cargoExecutable: String = run {
    val cargoHome = System.getenv("CARGO_HOME")?.takeIf { it.isNotBlank() }
        ?: File(System.getProperty("user.home"), ".cargo").absolutePath
    val exeName =
        if (System.getProperty("os.name").lowercase().contains("windows")) "cargo.exe" else "cargo"
    val exe = File(cargoHome, "bin/$exeName")
    if (exe.isFile) exe.absolutePath else "cargo"
}

// 引擎产物(T1 锁定口径:arm64-v8a + API 29 + release——-P 大写 29 不可省,
// 缺省 API 21 的目标缺 getifaddrs,链接失败)。
val engineProfile = "release"
val engineBinary: File
    get() = File(repoRoot, "target/aarch64-linux-android/$engineProfile/mtdssd")

// 引擎打包位:标准 jniLibs 目录(src/main/jniLibs 由 AGP 缺省拾取,无需
// sourceSets 挂接——AGP 9/Gradle 9 的 srcDir DSL 已废弃且在 kts 中升级为
// 编译错,故走缺省布局)。该目录整体 gitignore(生成物,不入库)。
val jniLibsArm64: File
    get() = File(projectDir, "src/main/jniLibs/arm64-v8a")

// AGP 9 的 SDK 目录访问面(android.sdkDirectory 已从 DSL 移除)。
val sdkDirectoryPath = androidComponents.sdkComponents.sdkDirectory.map { it.asFile.absolutePath }

// ── 更新源注入(T12,spec §6):镜像 M4 的构建期注入口径 ────────────────────
// 两个属性缺省空 = 应用内「检查更新」呈「未配置更新源」(fail-closed,与 M4
// 「更新未配置」同语义);覆盖途径(优先级高到低):命令行 -P >
// android/gradle.properties(本仓提交空值)> ~/.gradle/gradle.properties。
// 与服务器侧同源:dl 基址即 release workflow 的 vars.MTDSS_DL_BASE 形态
// (如 https://<VPS_IP>/dl,域名备案前不得硬编码 IP 入仓)。
fun updateProp(name: String): String = providers.gradleProperty(name).getOrElse("")

// Java 字符串字面量转义(BuildConfig 注入值须原样落到源码常量)。
fun javaStringLiteral(s: String): String = "\"" + s.replace("\\", "\\\\").replace("\"", "\\\"") + "\""

// ── release 签名(T12):keystore 用户离线保管(与 ed25519 更新签名密钥同
// 规格管理,spec §7),口令四件经 gradle 属性注入、绝不入仓;四项齐备才挂
// signingConfig,缺任一 assembleRelease 产未签名 APK(fail-visible)。
// CI 只 assembleDebug(AGP 自动生成的 debug keystore),不触本面。
val releaseStoreFile = providers.gradleProperty("MTDSS_RELEASE_STORE_FILE")
val releaseStorePassword = providers.gradleProperty("MTDSS_RELEASE_STORE_PASSWORD")
val releaseKeyAlias = providers.gradleProperty("MTDSS_RELEASE_KEY_ALIAS")
val releaseKeyPassword = providers.gradleProperty("MTDSS_RELEASE_KEY_PASSWORD")
val releaseSigningReady = listOf(
    releaseStoreFile, releaseStorePassword, releaseKeyAlias, releaseKeyPassword,
).all { it.isPresent }

android {
    namespace = "com.mtdss.android"
    compileSdk = 37

    // 使用本机实装的 build-tools(避免 AGP 按默认版本号向 dl.google.com 拉取,
    // 国内无代理直连不可达)
    buildToolsVersion = "36.0.0"

    defaultConfig {
        applicationId = "com.mtdss.android"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        // 引擎仅交叉编译 arm64-v8a;APK 限定同架构(其他 ABI 无产物可打)
        ndk { abiFilters += "arm64-v8a" }
        // 更新源(见文件头「更新源注入」段):BuildConfig 常量,App 启动即读,
        // 空串 = 未配置态(UpdateController fail-closed)。
        buildConfigField("String", "MTDSS_DL_BASE", javaStringLiteral(updateProp("MTDSS_DL_BASE")))
        buildConfigField("String", "MTDSS_UPDATE_PUBKEY", javaStringLiteral(updateProp("MTDSS_UPDATE_PUBKEY")))
    }

    signingConfigs {
        // 仅当四项属性齐备时注册(路径相对 android/ 或绝对;生成/保管见
        // scripts/android/README.md「release 签名」)。
        if (releaseSigningReady) {
            create("release") {
                storeFile = rootProject.file(releaseStoreFile.get())
                storePassword = releaseStorePassword.get()
                keyAlias = releaseKeyAlias.get()
                keyPassword = releaseKeyPassword.get()
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            if (releaseSigningReady) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }

    buildFeatures {
        // Compose 编译器由 AGP 9 内建 Kotlin 提供,无需独立插件
        compose = true
        // BuildConfig 常量面(T12 更新源注入;AGP 9 缺省不再生成 BuildConfig)
        buildConfig = true
    }

    testOptions {
        unitTests {
            // android.jar 桩默认对框架调用抛「not mocked」;T10 起
            // SourceProvider 等 app 级组件带 android.util.Log 直上 JVM 单测
            // (订阅/应答决策面),桩改回默认值(Log.x 返回 0)。被测类的
            // 其余框架面一律经注入缝(SafFace/RootsPersistence/SocketFactory),
            // 不受此开关影响。
            isReturnDefaultValues = true
        }
    }

    packaging {
        jniLibs {
            // 必须 true:引擎以 ProcessBuilder exec nativeLibraryDir 下的真实文件,
            // 需安装期解包(extractNativeLibs;false 时 .so 直存 APK 内 mmap,不可 exec)
            useLegacyPackaging = true
        }
    }

    // ── IPC 契约 golden 单一真源接线(T8)────────────────────────────────────
    // crates/ipc/contract/*.json(T3 生成,daemon 契约测试同源)挂为单元测试
    // 资源:ContractGoldenTest 遍历全量比对 Kotlin 客户端序列化/解析。
    // 目录以 android/app 的上级(仓库根)解析,与 Gradle 从仓库根或 android/
    // 哪里发起无关(Gradle 的 projectDir 恒为绝对路径)。
    sourceSets {
        getByName("test") {
            resources.srcDir(File(repoRoot, "crates/ipc/contract"))
        }
    }
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.material3)
    // 图标集(Refresh/Settings/Folder 等核心集;版本随 compose BOM)
    implementation("androidx.compose.material:material-icons-core")
    // FileProvider(更新 APK 拉起系统安装器,androidx.core;compose 面本就
    // 传递依赖它,此处显式声明用到的面)
    implementation(libs.androidx.core.ktx)
    implementation(libs.kotlinx.coroutines.android)
    // ed25519 验签(更新清单,M4 信任链;API 29 无 java.security Ed25519,
    // 见 libs.versions.toml bcprov 注释)
    implementation(libs.bouncycastle.bcprov)
    // JVM 单元测试:T8 IPC 客户端 + 契约 golden 套件(内存管道伪 server,
    // 无 Android 框架依赖;coroutines/bcprov 经 implementation 传递可见)
    testImplementation("junit:junit:4.13.2")
}

// ── 引擎构建任务 ───────────────────────────────────────────────────────────
// 两个变体任务同用一条 T1 锁定的命令(cargo 自身增量,重复执行近零成本)。
// Exec 完成(编译成功)后 doLast 拷贝产物为 libmtdssd.so。
// 注:NDK 由 cargo-ndk 经 ANDROID_HOME 自动定位(本机恰装 ndk/30.0.16248370)。
fun engineCommand(): List<String> = listOf(
    cargoExecutable, "ndk",
    "-t", "arm64-v8a",
    "-P", "29",
    "--", "build", "-p", "mtdss-daemon", "--release",
)

fun copyEngineToJniLibs(fromTask: String) {
    if (!engineBinary.isFile) {
        throw GradleException("$fromTask: 引擎产物缺失 ${engineBinary.absolutePath}")
    }
    copy {
        from(engineBinary)
        into(jniLibsArm64)
        rename { "libmtdssd.so" }
    }
    logger.lifecycle("$fromTask: ${engineBinary.absolutePath} -> ${File(jniLibsArm64, "libmtdssd.so").absolutePath}")
}

tasks.register<Exec>("cargoDaemonDebug") {
    group = "mtdss"
    description = "cargo ndk 交叉编译 mtdss-daemon(arm64-v8a/API29/release)并拷贝为 jniLibs 的 libmtdssd.so(debug 变体)"
    workingDir = repoRoot
    commandLine(engineCommand())
    // 本机进程环境常无 ANDROID_HOME(仅 SDK 落盘),显式取 AGP 解析出的 SDK 目录
    environment("ANDROID_HOME", sdkDirectoryPath)
    doLast { copyEngineToJniLibs("cargoDaemonDebug") }
}

tasks.register<Exec>("cargoDaemonRelease") {
    group = "mtdss"
    description = "cargo ndk 交叉编译 mtdss-daemon(arm64-v8a/API29/release)并拷贝为 jniLibs 的 libmtdssd.so(release 变体)"
    workingDir = repoRoot
    commandLine(engineCommand())
    environment("ANDROID_HOME", sdkDirectoryPath)
    doLast { copyEngineToJniLibs("cargoDaemonRelease") }
}

// assemble<Variant> → … → preBuild → cargoDaemonDebug,空仓克隆一次
// assembleDebug 即含引擎编译,无需先手工跑 cargo。两变体引擎同用一条
// 锁定命令(release 引擎),故统一挂 preBuild 而非逐变体挂接(AGP 9 已
// 移除旧 variant API applicationVariants,勿用)。
tasks.named("preBuild") {
    dependsOn("cargoDaemonDebug")
}
