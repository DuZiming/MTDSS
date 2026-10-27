package com.mtdss.android.ipc

import java.io.File

/**
 * IPC 契约 golden 目录解析与读取(crates/ipc/contract = 单一真源,与 daemon
 * 契约测试同源)。目录定位按序尝试:
 * 1. 从测试 CWD 逐级向上找 `crates/ipc/contract`(Gradle 从 android/ 或仓库根
 *    发起均可命中);
 * 2. 从测试类字节码位置向上找(异常 CWD 兜底);
 * 3. classpath 资源定位(build.gradle.kts 已把 golden 目录挂为 test 资源;
 *    仅 file: URL 可枚举)。
 */
object Goldens {

    fun dir(): File {
        val found = candidateRoots()
            .map { File(it, "crates/ipc/contract") }
            .firstOrNull { it.isDirectory }
            ?: error(
                "找不到 golden 目录 crates/ipc/contract(候选根:" +
                    candidateRoots().joinToString(", ") + ")"
            )
        return found
    }

    /** 读 golden 文件原始字节(含尾换行 = 线上帧原文)。 */
    fun read(name: String): ByteArray = dir().resolve(name).readBytes()

    /** 目录下全部文件名(排序后)。 */
    fun allNames(): List<String> =
        dir().listFiles()?.map { it.name }?.sorted()
            ?: error("golden 目录不可枚举: ${dir().absolutePath}")

    /**
     * golden 应答/事件行重写 id(其余内容原样——golden 本就是 serde_json 的
     * canonical 键序,经 [JVal.toCanonicalJson] 重排逐字节不变)。伪 server
     * 应答客户端任意 id 的调用时用。
     */
    fun withId(bytes: ByteArray, id: Long): ByteArray {
        val frame = JVal.parse(String(bytes, Charsets.UTF_8)) as JVal.Obj
        val rewritten = JVal.Obj(LinkedHashMap(frame.entries).apply { put("id", JVal.num(id)) })
        return (rewritten.toCanonicalJson() + "\n").toByteArray(Charsets.UTF_8)
    }

    private fun candidateRoots(): Sequence<File> {
        val starts = buildList {
            add(File(System.getProperty("user.dir") ?: "."))
            codeSourceDir()?.let { add(it) }
            classpathGoldenDir()?.let { add(it) }
        }
        return starts.asSequence()
            .map { it.absoluteFile }
            .flatMap { start -> generateSequence(start) { it.parentFile } }
            .distinct()
    }

    /** 测试类字节码所在目录(异常 CWD 兜底:build 下的 classes/intermediates)。 */
    private fun codeSourceDir(): File? = try {
        ContractGoldenTest::class.java.protectionDomain?.codeSource?.location?.toURI()?.let { File(it) }
    } catch (_: Exception) {
        null
    }

    /** classpath 上 golden 资源目录(build.gradle.kts sourceSets 挂接;仅 file: 形态可枚举)。 */
    private fun classpathGoldenDir(): File? = try {
        val url = ContractGoldenTest::class.java.classLoader?.getResource("hello.req.json")
            ?: return null
        if (url.protocol != "file") return null
        File(url.toURI()).parentFile
    } catch (_: Exception) {
        null
    }
}
