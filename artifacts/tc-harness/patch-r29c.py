# ═══ rendition.rs:viewer 时长 VOD 化(简化:仅全列+ENDLIST) ═══
p = "crates/daemon/src/preview/rendition.rs"
d = open(p, "rb").read().decode("utf-8")

BS = chr(92)
NL = BS + "n"

# 1) SessionState 增 duration_ms
old = """    /// 源端段尽(field 修:拉段遇段越界 = 源端自然完毕——playlist 据此
    /// 封 ENDLIST,时长锁定全长 + 播放端结束态)。
    completed: bool,
}"""
assert old in d, "SessionState completed"
new = """    /// 源端段尽(field 修:拉段遇段越界 = 源端自然完毕——playlist 据此
    /// 封 ENDLIST,时长锁定全长 + 播放端结束态)。
    completed: bool,
    /// 源总时长毫秒(r29:open 应答携带;0 = 未知 → EVENT 生长形)。
    duration_ms: u64,
}"""
d = d.replace(old, new, 1)

# 2) attach:存 duration
old = """            completed: false,
                session_id: resp.session_id,
                segment_ms: resp.segment_ms,
                mime: resp.mime,"""
assert old in d, "attach literal"
new = """            completed: false,
                duration_ms: resp.duration_ms,
                session_id: resp.session_id,
                segment_ms: resp.segment_ms,
                mime: resp.mime,"""
d = d.replace(old, new, 1)

# 3) playlist_m3u8:duration 已知 → 全列 + ENDLIST
old = """        let (origin, pulled, segment_ms, completed) = {
            let st = self.state.lock().unwrap();
            (st.origin, st.pulled, st.segment_ms, st.completed)
        };"""
assert old in d, "playlist tuple"
new = """        let (origin, pulled, segment_ms, completed, duration_ms) = {
            let st = self.state.lock().unwrap();
            (st.origin, st.pulled, st.segment_ms, st.completed, st.duration_ms)
        };"""
d = d.replace(old, new, 1)

fmt_line = '                s.push_str(&format!("#EXTINF:{secs:.3},' + NL + '{seg_prefix}/{i}.m4s?t={token}' + NL + '"));'
old = (
    "        if pulled >= 2 {\n"
    "            for i in origin..origin + pulled - 1 {\n"
    + fmt_line + "\n"
    "            }\n"
    "        }\n"
    "        if completed {\n"
    '            s.push_str("#EXT-X-ENDLIST' + NL + '");\n'
    "        }\n"
    "        s\n"
    "    }"
)
assert old in d, "playlist body"
new = (
    "        if duration_ms > 0 {\n"
    "            // r29 VOD 形:源总时长已知 → 全部段一次性列出 + ENDLIST——\n"
    "            // 时长自第一帧即全长、整条可 seek;未产段由段路由的有界等\n"
    "            // 待/远跳重开兜(hls.js 按需请求)。重开锚(origin>1)时自\n"
    "            // 锚列至片尾(查看端索引空间沿用)。\n"
    "            let total = u32::try_from(duration_ms.div_ceil(segment_ms as u64))\n"
    "                .unwrap_or(u32::MAX);\n"
    "            for i in origin..=total {\n"
    + fmt_line.replace("                ", "            ") + "\n"
    "            }\n"
    '            s.push_str("#EXT-X-ENDLIST' + NL + '");\n'
    "            return s;\n"
    "        }\n"
    "        if pulled >= 2 {\n"
    "            for i in origin..origin + pulled - 1 {\n"
    + fmt_line + "\n"
    "            }\n"
    "        }\n"
    "        if completed {\n"
    '            s.push_str("#EXT-X-ENDLIST' + NL + '");\n'
    "        }\n"
    "        s\n"
    "    }"
)
d = d.replace(old, new, 1)

open(p, "wb").write(d.encode("utf-8"))
print("viewer VOD done")
