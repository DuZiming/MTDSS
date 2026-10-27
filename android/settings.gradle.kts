// MTDSS Android 客户端 —— 独立 Gradle 工程(设计 §7:与 cargo workspace
// 平级、不入 workspace;ui/src-tauri 同款口径)。仓库根 = 本目录的上级。
//
// 国内网络口径(工程承诺,新机器开箱即用):
//   - Gradle 发行:腾讯镜像(gradle/wrapper/gradle-wrapper.properties)
//   - Maven 依赖:aliyun 镜像优先,google()/mavenCentral() 官方源兜底
//     (镜像缺件时兜底才会触达;Java 不吃 http_proxy 环境变量,镜像须直连可达)
pluginManagement {
    repositories {
        maven("https://maven.aliyun.com/repository/gradle-plugin")
        maven("https://maven.aliyun.com/repository/google")
        maven("https://maven.aliyun.com/repository/public")
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        maven("https://maven.aliyun.com/repository/google")
        maven("https://maven.aliyun.com/repository/public")
        google()
        mavenCentral()
    }
}

rootProject.name = "MTDSS"
include(":app")
