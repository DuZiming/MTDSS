import XCTest
import MTDSSKit

/// 传输列表页视图模型单测(T9 brief Step 1,先红后绿;镜像 android
/// TasksReducer/TasksViewModel 的已裁定语义):
/// - **分段过滤全四档**(spec §4.5:全部/接收/发送/调度;接收含被动 incoming);
/// - **状态行渲染映射**(进行中=进度+速度+百分比、中断=红字原因[error 空
///   兜底「已中断」]、完成=绿勾+时间[仅现场观测到 done 的行有时间]);
/// - **左滑动作集按状态三分**(进行中→[取消];中断/失败→[重新发起,清除];
///   完成/取消→[清除](v2-P5 T5b done 恢复清除入口);同签名在途行抑制
///   终态动作——双发/毁对端位图防线,android TasksReducer 同口径);
/// - **最新在最上**(daemon 快照升序反转 + 事件新行插顶);
/// - **event 增量更新 + 迟到丢弃**(终态粘滞:name/src/dst 事件帧不带保留
///   前值;会话行终态即消失);
/// - **清除已完成清全部有记录面终态**(done/中断/失败/已取消;T5b 全语义);
/// - **方向设备文案**(src/dst 设备段解析 + 目录名解析,缺失「…」);
/// - **通知触发条件**(仅完成且每作业一次;TransferCompletionAnnouncer)。
@MainActor
final class TransfersViewModelTests: XCTestCase {

    // MARK: 脚本助手

    private nonisolated static func jobJSON(_ j: TransferJob) -> JSONValue {
        .object([
            "id": .string(j.id),
            "kind": .string(j.kind),
            "name": .string(j.name),
            "state": .string(j.state),
            "files_done": .int(j.filesDone),
            "files_total": .int(j.filesTotal),
            "bytes_done": .int(j.bytesDone),
            "bytes_total": .int(j.bytesTotal),
            "speed_bps": .int(j.speedBps),
            "skipped": .int(j.skipped),
            "relay": .bool(j.relay),
            "src": j.src.map { .string($0) } ?? .null,
            "dst": j.dst.map { .string($0) } ?? .null,
            "error": j.error.map { .string($0) } ?? .null,
        ])
    }

    private nonisolated static func job(
        id: String, kind: String, name: String, state: String,
        filesDone: Int64 = 0, filesTotal: Int64 = 1,
        bytesDone: Int64 = 0, bytesTotal: Int64 = 1024,
        speedBps: Int64 = 0, skipped: Int64 = 0, relay: Bool = false,
        src: String? = nil, dst: String? = nil, error: String? = nil,
    ) -> TransferJob {
        TransferJob(
            id: id, kind: kind, name: name, state: state,
            filesDone: filesDone, filesTotal: filesTotal,
            bytesDone: bytesDone, bytesTotal: bytesTotal,
            speedBps: speedBps, skipped: skipped, relay: relay,
            src: src, dst: dst, error: error,
        )
    }

    /// daemon 侧脚本:transfer.list 按队列出应答(耗尽后重复末帧);三个
    /// 终态动作(cancel/clear/reinitiate)一律应 {} 并记录方法序。
    private final class ScriptBox: @unchecked Sendable {
        private let lock = NSLock()
        private var listResponses: [JSONValue]
        private var listIndex = 0
        private var listErrors: [String] = []
        private var errorIndex = 0
        private(set) var methods: [String] = []

        /// jobs 组逐组转 transfer.list 应答(耗尽后重复末组;空表 = 应空列表)。
        convenience init(jobs: [[TransferJob]], listErrors: [String] = []) {
            self.init(listResponses: jobs.isEmpty ? [.object(["jobs": .array([])])] : jobs.map {
                .object(["jobs": .array($0.map(TransfersViewModelTests.jobJSON))])
            }, listErrors: listErrors)
        }

        init(listResponses: [JSONValue], listErrors: [String] = []) {
            self.listResponses = listResponses
            self.listErrors = listErrors
        }

        /// 下一帧 transfer.list 应答(nil = 队列耗尽且无历史帧)。
        func nextList() -> JSONValue? {
            lock.lock()
            defer { lock.unlock() }
            if errorIndex < listErrors.count {
                defer { errorIndex += 1 }
                return .object(["__error__": .string(listErrors[errorIndex])])
            }
            guard !listResponses.isEmpty else { return nil }
            defer { listIndex += 1 }
            return listResponses[min(listIndex, listResponses.count - 1)]
        }

        func record(_ method: String) {
            lock.lock()
            defer { lock.unlock() }
            methods.append(method)
        }

        func snapshotMethods() -> [String] {
            lock.withLock { methods }
        }
    }

    private nonisolated static func responder(_ box: ScriptBox) -> FakeDaemon.LineHandler {
        { conn, line in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .string(let method)? = frame["method"],
                  case .number(let idRaw)? = frame["id"] else { return }
            switch method {
            case "transfer.list":
                guard let result = box.nextList() else { return }
                let line: String
                if case .object(let obj) = result,
                   let err = obj["__error__"],
                   case .string(let message) = err {
                    line = JSONValue.object([
                        "id": .number(idRaw),
                        "error": .object(["code": .number("1"), "message": .string(message)]),
                    ]).canonical
                } else {
                    line = Self.envelope(idRaw: idRaw, result: result)
                }
                conn.sendLine(Data((line + "\n").utf8))
            case "transfer.cancel", "transfer.clear", "transfer.reinitiate":
                box.record(method)
                conn.sendLine(Data((Self.envelope(idRaw: idRaw, result: .object([:])) + "\n").utf8))
            default:
                break
            }
        }
    }

    private nonisolated static func envelope(idRaw: String, result: JSONValue) -> String {
        JSONValue.object([
            "id": .number(idRaw),
            "result": result,
        ]).canonical
    }

    private func makeVM(_ fake: FakeDaemon) -> TransfersViewModel {
        TransfersViewModel(repo: MtdssRepository(client: IpcClient(endpoint: fake)))
    }

    // MARK: 种子:最新在最上 + 终态会话行滤除

    func testSeedReversesDaemonOrderNewestFirstAndDropsTerminalSessionRows() async throws {
        let fake = FakeDaemon()
        // daemon 快照 = job_seq 升序(最旧在前);done 的 incoming 是会话行
        let box = ScriptBox(jobs: [[
            Self.job(id: "1", kind: "upload", name: "旧发送", state: "running", src: "/a", dst: "dev-a:/inbox"),
            Self.job(id: "2", kind: "incoming", name: "dev-b", state: "done", src: "dev-b", dst: "/dir"),
            Self.job(id: "3", kind: "download", name: "中途失败", state: "failed", src: "dev-a:/x.bin", error: "目标设备离线"),
            Self.job(id: "4", kind: "upload", name: "最新发送", state: "queued", src: "/c", dst: "dev-a:/inbox"),
        ]])
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()

        XCTAssertNil(vm.errorText)
        XCTAssertEqual(vm.rows.map(\.id), ["4", "3", "1"], "最新在最上;终态会话行(incoming done)滤除")
    }

    // MARK: 分段过滤全四档(接收含被动 incoming)

    func testFilterFourSegments() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox(jobs: [[
            Self.job(id: "1", kind: "upload", name: "发送", state: "done"),
            Self.job(id: "2", kind: "download", name: "接收", state: "done"),
            Self.job(id: "3", kind: "incoming", name: "被动收", state: "running"),
            Self.job(id: "4", kind: "remote_copy", name: "调度", state: "failed"),
        ]])
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()

        func ids(_ f: TransferFilter) -> [String] {
            vm.filter = f
            return vm.visibleRows.map(\.id)
        }
        XCTAssertEqual(ids(.all), ["4", "3", "2", "1"])
        XCTAssertEqual(ids(.send), ["1"], "发送档 = upload")
        XCTAssertEqual(ids(.receive), ["3", "2"], "接收档 = download + incoming")
        XCTAssertEqual(ids(.schedule), ["4"], "调度档 = remote_copy")
    }

    // MARK: 状态行渲染映射

    func testStateRenderMapping() {
        XCTAssertEqual(TransfersFormat.stateText("running", error: nil), "进行中")
        XCTAssertEqual(TransfersFormat.stateText("queued", error: nil), "进行中")
        XCTAssertEqual(TransfersFormat.stateText("paused", error: nil), "已暂停")
        XCTAssertEqual(TransfersFormat.stateText("publishing", error: nil), "保存中")
        XCTAssertEqual(TransfersFormat.stateText("interrupted", error: nil), "已中断")
        XCTAssertEqual(TransfersFormat.stateText("failed", error: nil), "失败")
        XCTAssertEqual(TransfersFormat.stateText("failed", error: "保存失败(2 项): 磁盘满"), "保存失败")
        XCTAssertEqual(TransfersFormat.stateText("done", error: nil), "完成")
        XCTAssertEqual(TransfersFormat.stateText("cancelled", error: nil), "已取消")

        // 中断红字:引擎 error 在场即原因,空则兜底「已中断」(T9 简报语义)
        XCTAssertEqual(TransfersFormat.causeText("interrupted", error: "目标设备离线"), "已中断 · 目标设备离线")
        XCTAssertEqual(TransfersFormat.causeText("interrupted", error: nil), "已中断")
        XCTAssertEqual(TransfersFormat.causeText("failed", error: "信令未在线"), "信令未在线")
        XCTAssertEqual(TransfersFormat.causeText("failed", error: nil), "失败")
        XCTAssertNil(TransfersFormat.causeText("running", error: nil), "进行中行无红因")
    }

    func testSpeedAndProgressFormatting() {
        XCTAssertEqual(TransfersFormat.speedText(0), "0 B/s")
        XCTAssertEqual(TransfersFormat.speedText(512), "512 B/s")
        XCTAssertEqual(TransfersFormat.speedText(2_200_000), "2.1 MB/s")
        XCTAssertEqual(TransfersFormat.progressRatio(bytesDone: 450, bytesTotal: 1000) ?? -1, 0.45, accuracy: 0.0001)
        XCTAssertNil(TransfersFormat.progressRatio(bytesDone: 10, bytesTotal: 0), "总量未知 → 不确定态进度条")
        // final review I2:name 由 daemon 单源计算(多项已带「等 N 项」单
        // 后缀),显示名直读不再自拼「· N 项」双后缀;空名回退 id
        XCTAssertEqual(TransfersFormat.displayName(Self.job(id: "1", kind: "upload", name: "样本.bin", state: "running", filesTotal: 1)), "样本.bin")
        XCTAssertEqual(TransfersFormat.displayName(Self.job(id: "1", kind: "upload", name: "报告.pdf 等 3 项", state: "running", filesTotal: 3)), "报告.pdf 等 3 项")
        XCTAssertEqual(TransfersFormat.displayName(Self.job(id: "1", kind: "upload", name: "", state: "running")), "1", "空名回退 id")
    }

    // MARK: 跳过计数可见化(R3 真机轮 #3/#6:同名跳过的重复机制上副文)

    func testSkippedTextProjection() {
        XCTAssertNil(TransfersFormat.skippedText(0), "无跳过不显示")
        XCTAssertNil(TransfersFormat.skippedText(-1), "负值防御不显示")
        XCTAssertEqual(TransfersFormat.skippedText(1), "跳过 1 项")
        XCTAssertEqual(TransfersFormat.skippedText(12), "跳过 12 项")
    }

    /// skipped 全链保真:transfer.list 种子与 event.transfer 帧携带的
    /// skipped 都落行(job 字段直取,无投影损失)——完成行副文组装由
    /// TransferRowView 消费本投影(视图面)。
    func testSkippedSurvivesSeedAndEventFrames() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox(jobs: [[
            Self.job(id: "1", kind: "upload", name: "重复发送", state: "done", skipped: 3),
        ]])
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()
        XCTAssertEqual(vm.rows.first?.skipped, 3)

        // 新行经事件帧入场(done 非 session kind 建行),skipped 随帧保留
        _ = vm.apply(TransferProgress(
            id: "2", kind: "download", state: "done",
            filesDone: 2, filesTotal: 5, bytesDone: 10, bytesTotal: 10,
            speedBps: 0, skipped: 2, relay: false, error: nil,
        ))
        XCTAssertEqual(vm.rows.first(where: { $0.id == "2" })?.skipped, 2)
    }

    // MARK: 左滑动作集按状态(+同签名在途抑制)

    func testActionsByStateThreeWays() {
        func acts(_ state: String, kind: String = "upload") -> [TransferRowAction] {
            TransfersFormat.actions(for: Self.job(id: "x", kind: kind, name: "n", state: state), rows: [])
        }
        XCTAssertEqual(acts("queued"), [.cancel])
        XCTAssertEqual(acts("running"), [.cancel])
        XCTAssertEqual(acts("paused"), [.cancel], "spec §4.5 进行中仅列取消,pause/resume 不上此页")
        XCTAssertEqual(acts("publishing"), [.cancel])
        XCTAssertEqual(acts("interrupted"), [.reinitiate, .clear])
        XCTAssertEqual(acts("failed"), [.reinitiate, .clear])
        XCTAssertEqual(acts("cancelled"), [.clear])
        // T5b(Ruling K):done 恢复清除入口(引擎侧放开,「清除已完成」全语义)
        XCTAssertEqual(acts("done"), [.clear])
        XCTAssertEqual(acts("running", kind: "incoming"), [.cancel], "会话级来件进行中可取消")
        XCTAssertEqual(acts("interrupted", kind: "incoming"), [], "会话行无记录面,终态无动作(防御)")
        XCTAssertEqual(acts("done", kind: "incoming"), [], "会话行 done 同无动作(无记录面)")
    }

    func testSiblingInFlightSuppressesTerminalActions() {
        let inFlight = Self.job(id: "2", kind: "upload", name: "同件重发", state: "running", src: "/a", dst: "dev-a:/inbox")
        let staleFailed = Self.job(id: "1", kind: "upload", name: "旧失败行", state: "failed", src: "/a", dst: "dev-a:/inbox")
        XCTAssertEqual(TransfersFormat.actions(for: staleFailed, rows: [staleFailed, inFlight]), [],
                       "同 kind+src+dst 在途行在场 → 终态动作抑制(双发/毁位图防线)")

        let unrelated = Self.job(id: "3", kind: "download", name: "别的", state: "running", src: "dev-a:/y", dst: "/dir")
        XCTAssertEqual(TransfersFormat.actions(for: staleFailed, rows: [staleFailed, unrelated]), [.reinitiate, .clear],
                       "异签名在途行不抑制")
    }

    // MARK: 方向设备文案

    func testDirectionTextMapping() {
        let names = ["dev-a": "公司台式机", "dev-b": "家里的 iPhone"]
        func dir(_ j: TransferJob) -> String { TransfersFormat.directionText(j, resolve: { names[$0] ?? $0 }) }

        XCTAssertEqual(dir(Self.job(id: "1", kind: "upload", name: "n", state: "running", dst: "dev-a:C:/inbox")), "本机 → 公司台式机")
        XCTAssertEqual(dir(Self.job(id: "2", kind: "download", name: "n", state: "running", src: "dev-a:/x/a.pdf")), "公司台式机 → 本机")
        XCTAssertEqual(dir(Self.job(id: "3", kind: "remote_copy", name: "n", state: "running", src: "dev-a:/x", dst: "dev-b:C:/dir")), "公司台式机 → 家里的 iPhone")
        XCTAssertEqual(dir(Self.job(id: "4", kind: "incoming", name: "dev-a", state: "running", src: "dev-a")), "公司台式机 → 本机", "被动来件 src = 裸设备 id")
        XCTAssertEqual(dir(Self.job(id: "5", kind: "upload", name: "n", state: "running", dst: "dev-unknown:/d")), "本机 → dev-unknown", "目录缺名回退裸 id")
        XCTAssertEqual(dir(Self.job(id: "6", kind: "upload", name: "n", state: "running", dst: nil)), "本机 → …", "方向面缺失占位 …")
    }

    // MARK: event 增量更新 + 迟到丢弃(终态粘滞)

    func testApplyEventMergesInPlaceAndInsertsNewRowsOnTop() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox(jobs: [[
            Self.job(id: "1", kind: "upload", name: "报告.pdf", state: "running",
                     bytesDone: 100, bytesTotal: 1000, speedBps: 0, src: "/r", dst: "dev-a:/inbox"),
        ]])
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()

        // 进度帧(不带 name/src/dst):原位合并,元数据保留
        vm.apply(TransferProgress(
            id: "1", kind: "upload", state: "running",
            filesDone: 0, filesTotal: 1, bytesDone: 450, bytesTotal: 1000,
            speedBps: 2_200_000, skipped: 0, relay: false, error: nil,
        ))
        XCTAssertEqual(vm.rows.count, 1)
        XCTAssertEqual(vm.rows[0].bytesDone, 450)
        XCTAssertEqual(vm.rows[0].speedBps, 2_200_000)
        XCTAssertEqual(vm.rows[0].name, "报告.pdf", "事件帧不带 name,保留前值")
        XCTAssertEqual(vm.rows[0].src, "/r")
        XCTAssertEqual(vm.rows[0].dst, "dev-a:/inbox")

        // 全新 id → 插顶(最新在最上)
        vm.apply(TransferProgress(
            id: "2", kind: "download", state: "running",
            filesDone: 0, filesTotal: 1, bytesDone: 10, bytesTotal: 100,
            speedBps: 1, skipped: 0, relay: false, error: nil,
        ))
        XCTAssertEqual(vm.rows.map(\.id), ["2", "1"])
    }

    func testTerminalStickinessDropsLateFrames() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox(jobs: [[
            Self.job(id: "1", kind: "upload", name: "n", state: "done", bytesDone: 1000, bytesTotal: 1000),
        ]])
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()

        // 终态后的迟到进度帧整体忽略(终态粘滞)
        vm.apply(TransferProgress(
            id: "1", kind: "upload", state: "running",
            filesDone: 0, filesTotal: 1, bytesDone: 1, bytesTotal: 1000,
            speedBps: 5, skipped: 0, relay: false, error: nil,
        ))
        XCTAssertEqual(vm.rows[0].state, "done")
        XCTAssertEqual(vm.rows[0].bytesDone, 1000, "迟到帧不回退终态")
    }

    func testSessionRowDisappearsOnTerminalEvent() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox(jobs: [[
            Self.job(id: "9", kind: "incoming", name: "dev-b", state: "running", src: "dev-b"),
        ]])
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()
        XCTAssertEqual(vm.rows.count, 1)

        vm.apply(TransferProgress(
            id: "9", kind: "incoming", state: "done",
            filesDone: 1, filesTotal: 1, bytesDone: 100, bytesTotal: 100,
            speedBps: 0, skipped: 0, relay: false, error: nil,
        ))
        XCTAssertTrue(vm.rows.isEmpty, "会话行终态即消失(daemon 无记录面,不清即幽灵)")
    }

    // MARK: 完成时间(绿勾+时间;仅现场观测到 done 的行)

    func testCompletionTimeObservedOnlyForLiveDone() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox(jobs: [[
            Self.job(id: "seeded", kind: "upload", name: "历史完成", state: "done"),
            Self.job(id: "live", kind: "upload", name: "现场完成", state: "running"),
        ]])
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()

        XCTAssertNil(vm.completionTime(of: "seeded"), "种子已完成的行无可靠时间,不编造")
        vm.apply(TransferProgress(
            id: "live", kind: "upload", state: "done",
            filesDone: 1, filesTotal: 1, bytesDone: 1024, bytesTotal: 1024,
            speedBps: 0, skipped: 0, relay: false, error: nil,
        ))
        XCTAssertNotNil(vm.completionTime(of: "live"), "现场观测到 done → 记录时间")
    }

    // MARK: 行动作 → daemon 帧

    func testCancelAndReinitiateAndClearSendFrames() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox(listResponses: [.object(["jobs": .array([])])])
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()

        await vm.cancel("7")
        await vm.reinitiate("8")
        await vm.clear("9")
        let methods = box.snapshotMethods()
        XCTAssertEqual(methods, ["transfer.cancel", "transfer.reinitiate", "transfer.clear"])
        XCTAssertNil(vm.lastActionError)
    }

    func testClearCompletedClearsOnlyEndedClearableRows() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox(jobs: [[
            Self.job(id: "done-1", kind: "upload", name: "完1", state: "done", src: "/a1", dst: "dev-a:/inbox"),
            Self.job(id: "failed-1", kind: "download", name: "败1", state: "failed", src: "dev-a:/f1"),
            Self.job(id: "cancel-1", kind: "upload", name: "消1", state: "cancelled", src: "/c1", dst: "dev-a:/inbox"),
            Self.job(id: "run-1", kind: "upload", name: "传1", state: "running", src: "/r1", dst: "dev-a:/inbox"),
            Self.job(id: "done-2", kind: "download", name: "完2", state: "done", src: "dev-a:/d2"),
            Self.job(id: "intr-1", kind: "download", name: "断1", state: "interrupted", src: "dev-a:/i1"),
        ]])
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()

        await vm.clearCompleted()
        let methods = box.snapshotMethods()
        XCTAssertEqual(Set(methods), ["transfer.clear"], "清除已完成只发 transfer.clear")
        XCTAssertEqual(methods.filter { $0 == "transfer.clear" }.count, 5,
                       "全语义(v2-P5 T5b):done×2 + 中断/失败/已取消;进行中不动")
    }

    func testLoadErrorSurfacesAndRecovers() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox(
            jobs: [[
                Self.job(id: "1", kind: "upload", name: "n", state: "running"),
            ]],
            listErrors: ["boom"],
        )
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()
        XCTAssertNotNil(vm.errorText)
        XCTAssertTrue(vm.rows.isEmpty)

        await vm.load() // 重试成功 → 错误态收口
        XCTAssertNil(vm.errorText)
        XCTAssertEqual(vm.rows.map(\.id), ["1"])
    }

    // MARK: 通知触发条件(仅完成;每作业一次)

    func testCompletionAnnouncerFiresOncePerJobOnDone() {
        var announcer = TransferCompletionAnnouncer()

        func frame(id: String, state: String) -> TransferProgress {
            TransferProgress(
                id: id, kind: "upload", state: state,
                filesDone: 1, filesTotal: 1, bytesDone: 10, bytesTotal: 10,
                speedBps: 0, skipped: 0, relay: false, error: nil,
            )
        }

        XCTAssertNil(announcer.announce(frame(id: "a", state: "running")), "进行中不通知")
        let hit = announcer.announce(frame(id: "a", state: "done"))
        XCTAssertEqual(hit?.id, "a", "完成沿通知")
        XCTAssertNil(announcer.announce(frame(id: "a", state: "done")), "迟到重复帧不重复通知")
        XCTAssertEqual(announcer.announce(frame(id: "b", state: "done"))?.id, "b", "其他作业照常通知")
        XCTAssertNil(announcer.announce(frame(id: "c", state: "failed")), "仅完成通知(spec §3)")
    }
}
