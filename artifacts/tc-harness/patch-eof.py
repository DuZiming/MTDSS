import re

p = "crates/daemon/src/preview/rendition.rs"
d = open(p, "rb").read().decode("utf-8")

BS = chr(92)  # backslash

# 1) SessionState 增 completed
old = """    /// init 段缓存代次(== gen 时盘上 vseg0 属当前会话;远跳重开 gen 翻
    /// 转即作废旧 init——重开换管线 = 旧 moov 作废,下次拉取以当前会话
    /// init 覆写缓存件,MAP 面字节恒当前。缓存无删除 API,代次判据替代)。
    init_gen: u64,
}"""
new = """    /// init 段缓存代次(== gen 时盘上 vseg0 属当前会话;远跳重开 gen 翻
    /// 转即作废旧 init——重开换管线 = 旧 moov 作废,下次拉取以当前会话
    /// init 覆写缓存件,MAP 面字节恒当前。缓存无删除 API,代次判据替代)。
    init_gen: u64,
    /// 源端段尽(field 修:拉段遇段越界 = 源端自然完毕——playlist 据此
    /// 封 ENDLIST,时长锁定全长 + 播放端结束态)。
    completed: bool,
}"""
assert old in d, "SessionState"
d = d.replace(old, new, 1)

# 2) attach 的 SessionState 字面量补 false
m = re.search(r"state: Mutex::new\(SessionState \{", d)
assert m
idx = m.end()
d = d[:idx] + "\n            completed: false," + d[idx:]

# 3) pull_segments:Range 止步时置 completed
old = (
    "                Err(face) => match classify_seg_face(&face) {\n"
    "                    SegFace::Pending | SegFace::Range => break, // 止步:生产未至/段尽\n"
    "                    SegFace::Failed(c, m) => return Err((c, m)), // M5:失败勿吞\n"
    "                },\n"
    "            }\n"
    "        }\n"
    "        Ok(self.pulled_now())\n"
    "    }"
)
new = (
    "                Err(face) => match classify_seg_face(&face) {\n"
    "                    SegFace::Pending => break, // 止步:生产未至\n"
    "                    // field 修:段越界 = 源端自然完毕(0x0C eof 后引擎改答\n"
    "                    // 越界)——记段尽,playlist 封 ENDLIST\n"
    "                    SegFace::Range => {\n"
    "                        self.state.lock().unwrap().completed = true;\n"
    "                        break;\n"
    "                    }\n"
    "                    SegFace::Failed(c, m) => return Err((c, m)), // M5:失败勿吞\n"
    "                },\n"
    "            }\n"
    "        }\n"
    "        Ok(self.pulled_now())\n"
    "    }"
)
assert old in d, "pull break"
d = d.replace(old, new, 1)

# 4) playlist_m3u8:completed 时封 ENDLIST(注意源码里是字面 \n 两字符)
NL = BS + "n"
old = (
    "        let (origin, pulled, segment_ms) = {\n"
    "            let st = self.state.lock().unwrap();\n"
    "            (st.origin, st.pulled, st.segment_ms)\n"
    "        };"
)
new = (
    "        let (origin, pulled, segment_ms, completed) = {\n"
    "            let st = self.state.lock().unwrap();\n"
    "            (st.origin, st.pulled, st.segment_ms, st.completed)\n"
    "        };"
)
assert old in d, "playlist tuple"
d = d.replace(old, new, 1)

fmt_line = '                s.push_str(&format!("#EXTINF:{secs:.3},' + NL + '{seg_prefix}/{i}.m4s?t={token}' + NL + '"));'
old = (
    "        if pulled >= 2 {\n"
    "            for i in origin..origin + pulled - 1 {\n"
    + fmt_line + "\n"
    "            }\n"
    "        }\n"
    "        s\n"
    "    }"
)
new = (
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
assert old in d, "playlist endlist"
d = d.replace(old, new, 1)

open(p, "wb").write(d.encode("utf-8"))
print("viewer completed/ENDLIST done")
