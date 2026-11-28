# ═══ rendition.rs:r29 引擎面(mgr/ctrl/factory/transcoder/viewer VOD) ═══
p = "crates/daemon/src/preview/rendition.rs"
d = open(p, "rb").read().decode("utf-8")

BS = chr(92)
NL = BS + "n"

# 1) Transcoder trait 增 duration_ms 缺省面
old = "pub(crate) trait Transcoder: Send + Sync {"
assert old in d, "trait"
new = """pub(crate) trait Transcoder: Send + Sync {
    /// 源总时长毫秒(r29:open 回执携带;缺省 0 = 未知——查看端落
    /// EVENT 生长形)。
    fn duration_ms(&self) -> u64 {
        0
    }
"""
d = d.replace(old, new, 1)

# 2) AndroidTranscoder 字段 + 覆写
old = """struct AndroidTranscoder {
    bridge: std::sync::Arc<crate::jobs::RenditionBridge>,
    session: u64,
    serve_dir: PathBuf,
}"""
assert old in d, "transcoder struct"
new = """struct AndroidTranscoder {
    bridge: std::sync::Arc<crate::jobs::RenditionBridge>,
    session: u64,
    /// 源总时长毫秒(r29:open 回执携带;0 = 未知)。
    duration_ms: u64,
    serve_dir: PathBuf,
}"""
d = d.replace(old, new, 1)

old = """#[cfg(unix)]
impl Transcoder for AndroidTranscoder {
    fn segment(&self, index: u32) -> io::Result<Box<dyn io::Read + Send>> {"""
assert old in d, "impl"
new = """#[cfg(unix)]
impl Transcoder for AndroidTranscoder {
    fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    fn segment(&self, index: u32) -> io::Result<Box<dyn io::Read + Send>> {"""
d = d.replace(old, new, 1)

# 3) bridge_backend 工厂:解构 (session, duration)
old = """        match opened {
            Some(session) => {
                // 源面随手柄落放(桥后端不经 FileSource 读源——App 按
                // vpath 自行开源转码;source 入参仅供签名一致性/桌面实现)
                drop(source);
                Some(Arc::new(AndroidTranscoder {
                    bridge: Arc::clone(&bridge),
                    session,
                    serve_dir: serve_dir.clone(),
                }) as Arc<dyn Transcoder>)
            }"""
assert old in d, "factory"
new = """        match opened {
            Some((session, duration_ms)) => {
                // 源面随手柄落放(桥后端不经 FileSource 读源——App 按
                // vpath 自行开源转码;source 入参仅供签名一致性/桌面实现)
                drop(source);
                Some(Arc::new(AndroidTranscoder {
                    bridge: Arc::clone(&bridge),
                    session,
                    duration_ms,
                    serve_dir: serve_dir.clone(),
                }) as Arc<dyn Transcoder>)
            }"""
d = d.replace(old, new, 1)

# 4) SessionRec 增 duration_ms
old = """struct SessionRec {
    backend: Arc<dyn Transcoder>,"""
assert old in d, "SessionRec"
new = """struct SessionRec {
    backend: Arc<dyn Transcoder>,
    /// 源总时长毫秒(r29:后端自 open 回执携;0 = 未知)。
    duration_ms: u64,"""
d = d.replace(old, new, 1)

# 5) mgr.open 返回 (id, duration)
old = """    ) -> Result<u64, OpenError> {
        self.sweep();
        let factory = Arc::clone(&self.factory.lock().unwrap());
        let backend = (factory)(source, path, start_at_ms).ok_or(OpenError::Unsupported)?;
        let mut st = self.inner.lock().unwrap();"""
assert old in d, "mgr open head"
new = """    ) -> Result<(u64, u64), OpenError> {
        self.sweep();
        let factory = Arc::clone(&self.factory.lock().unwrap());
        let backend = (factory)(source, path, start_at_ms).ok_or(OpenError::Unsupported)?;
        let duration_ms = backend.duration_ms();
        let mut st = self.inner.lock().unwrap();"""
d = d.replace(old, new, 1)

old = """        st.sessions.insert(
            id,
            SessionRec {
                backend,
                segment_ms: config.segment_ms,
                mime: SEGMENT_MIME,
                last_used: (self.now)(),
            },
        );
        Ok(id)
    }"""
assert old in d, "mgr open tail"
new = """        st.sessions.insert(
            id,
            SessionRec {
                backend,
                duration_ms,
                segment_ms: config.segment_ms,
                mime: SEGMENT_MIME,
                last_used: (self.now)(),
            },
        );
        Ok((id, duration_ms))
    }"""
d = d.replace(old, new, 1)

# 6) ctrl 应答:解构 + 携字段
old = """    match opened {
        Ok(Ok(session_id)) => CtrlReply::Frame(CtrlResponse {
            version: mtdss_proto::CTRL_VERSION,
            ok: true,
            body: Some(ctrl_response::Body::RenditionOpen(RenditionOpenResponse {
                session_id,
                segment_ms: RenditionConfig::SEGMENT_MS,
                mime: SEGMENT_MIME.into(),
            })),
        }),"""
assert old in d, "ctrl reply"
new = """    match opened {
        Ok(Ok((session_id, duration_ms))) => CtrlReply::Frame(CtrlResponse {
            version: mtdss_proto::CTRL_VERSION,
            ok: true,
            body: Some(ctrl_response::Body::RenditionOpen(RenditionOpenResponse {
                session_id,
                segment_ms: RenditionConfig::SEGMENT_MS,
                mime: SEGMENT_MIME.into(),
                duration_ms,
            })),
        }),"""
d = d.replace(old, new, 1)

# 7) RenditionFetch::open 的响应解构:session/segment_ms/mime → +duration
old = """    async fn open("""
print("(fetch open at:", d.find("    async fn open("), ")")

open(p, "wb").write(d.encode("utf-8"))
print("rendition.rs r29 core done (fetch/viewer 待查)")
