// 顶层构建脚本:仅声明插件版本(实际 apply 在 app 模块)。Kotlin 编译器
// 由 AGP 9 内建(built-in Kotlin),无 kotlin-android 插件可声明;Compose
// 编译器插件单独引入(app 模块)。
plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.kotlin.compose) apply false
}
