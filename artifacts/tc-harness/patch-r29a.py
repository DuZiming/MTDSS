# ═══ jobs.rs:RenditionBridge.open 返回 (session, duration_ms) ═══
p = "crates/daemon/src/jobs.rs"
d = open(p, "rb").read().decode("utf-8")

old = "    pending_open: Mutex<HashMap<u64, oneshot::Sender<Result<(), String>>>>,"
assert old in d, "pending_open decl"
d = d.replace(old, "    pending_open: Mutex<HashMap<u64, oneshot::Sender<Result<u64, String>>>>,", 1)

old = """                if let Some(tx) = lock_ok(&self.pending_open).remove(&request_id) {
                    // 在册登记随回执落定(先于 open() 返回的后续动作——
                    // 回执窗内的段回告已在册,无竞窗)
                    lock_ok(&self.live).insert(request_id, ());
                    tx.send(Ok(())).is_ok()
                } else if let Some(tx) = lock_ok(&self.pending_cancel).remove(&request_id) {"""
new = """                if let Some(tx) = lock_ok(&self.pending_open).remove(&request_id) {
                    // 在册登记随回执落定(先于 open() 返回的后续动作——
                    // 回执窗内的段回告已在册,无竞窗)。r29:回执可携
                    // duration_ms(App 源总时长探针;缺省/旧 App = 0 未知)
                    let duration_ms = payload
                        .get("duration_ms")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    lock_ok(&self.live).insert(request_id, ());
                    tx.send(Ok(duration_ms)).is_ok()
                } else if let Some(tx) = lock_ok(&self.pending_cancel).remove(&request_id) {"""
assert old in d, "ack arm"
d = d.replace(old, new, 1)

old = """    pub(crate) async fn open(&self, path: &str, start_at_ms: u64) -> Result<u64, String> {"""
new = """    /// r29:回执携 duration_ms(App 总时长探针;0 = 未知)——返回
    /// (会话键, 源总时长毫秒)。
    pub(crate) async fn open(&self, path: &str, start_at_ms: u64) -> Result<(u64, u64), String> {"""
assert old in d, "open sig"
d = d.replace(old, new, 1)

old = """        match outcome {
            Ok(Ok(Ok(()))) => Ok(request_id),
            Ok(Ok(Err(reason))) => Err(reason),"""
new = """        match outcome {
            Ok(Ok(Ok(duration_ms))) => Ok((request_id, duration_ms)),
            Ok(Ok(Err(reason))) => Err(reason),"""
assert old in d, "open match"
d = d.replace(old, new, 1)

old = """        let session = waiter.await.unwrap().expect("回执后 open 应落定");
        assert_eq!(session, id, "会话键 = open 的 request_id");"""
new = """        let (session, duration) = waiter.await.unwrap().expect("回执后 open 应落定");
        assert_eq!(session, id, "会话键 = open 的 request_id");
        assert_eq!(duration, 0, "缺省 duration_ms = 0(旧 App 形)");

        // r29:回执携 duration_ms → open 返回元组右值
        {
            let w2 = {
                let bridge = bridge.clone();
                tokio::spawn(async move { bridge.open("/收件箱/m2.mp4", 0).await })
            };
            let ev2 = rx.recv().await.unwrap();
            let id2 = ev2.params["request_id"].as_u64().unwrap();
            assert!(
                bridge.deliver(
                    id2,
                    &json!({ "duration_ms": 26_000, "ok": true, "request_id": id2 }),
                ),
                "携时长回执应受理"
            );
            let (s2, d2) = w2.await.unwrap().expect("携时长 open 落定");
            assert_eq!((s2, d2), (id2, 26_000), "duration_ms 透传");
        }"""
assert old in d, "test session"
d = d.replace(old, new, 1)

open(p, "wb").write(d.encode("utf-8"))
print("jobs.rs r29 done")
