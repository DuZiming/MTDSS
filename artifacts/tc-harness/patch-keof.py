p = "android/app/src/test/java/com/mtdss/android/serve/RenditionResponderTest.kt"
d = open(p, "rb").read().decode("utf-8")

Q3 = chr(34) * 3
Q1 = chr(34)

old = (
    '            assertEquals("视频.mp4", env.factory.calls[0].displayName)\n'
    "            // fd 所有权:工厂返回即 release(响应器兜底)\n"
    '            assertEquals("源用毕释放", 1, env.saf.releases)'
)
eof_json = "{" + Q1 + "eof" + Q1 + ":true," + Q1 + "ok" + Q1 + ":true," + Q1 + "request_id" + Q1 + ":9}"
new = (
    '            assertEquals("视频.mp4", env.factory.calls[0].displayName)\n'
    "            // 帧 3:0x0C eof 形钉面({eof, ok, request_id})\n"
    "            assertEquals(FdWire.KIND_RENDITION_DATA, conn.frames[3].kind)\n"
    "            assertEquals(\n"
    "                " + Q3 + eof_json + Q3 + ",\n"
    "                conn.frames[3].payload.toCanonicalJson(),\n"
    "            )\n"
    "            // fd 所有权:工厂返回即 release(响应器兜底)\n"
    '            assertEquals("源用毕释放", 1, env.saf.releases)'
)
assert old in d, "anchor"
d = d.replace(old, new, 1)
open(p, "wb").write(d.encode("utf-8"))
print("kotlin eof assert added")
