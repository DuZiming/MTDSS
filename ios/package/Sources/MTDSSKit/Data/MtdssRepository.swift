/// MtdssRepository — 登录/设备/浏览/传输/任务查询的 async 门面(UI 各屏的
/// 数据源;镜像 android IpcClient 类型化面)。方法名 = 契约 RPC 方法名
/// (Ruling A:hello/status/devices/login.*/list/stat/transfer.*)。实现薄:
/// 每方法一调用一类型化解析。

import Foundation

public struct MtdssRepository: Sendable {
    private let client: IpcClient

    public init(client: IpcClient) {
        self.client = client
    }

    public init(endpoint: any SocketEndpoint) {
        self.client = IpcClient(endpoint: endpoint)
    }

    /// 底层客户端(事件订阅/原始调用/生命周期管理)。
    public var ipc: IpcClient { client }

    /// 服务端推送事件(单消费;多方订阅经 EventHub)。
    public var events: AsyncStream<IpcEvent> { client.events }

    // MARK: 基础

    public func hello() async throws -> HelloResult {
        try HelloResult(await client.send(Ops.hello))
    }

    public func status() async throws -> StatusResult {
        try StatusResult(await client.send(Ops.status))
    }

    public func devices() async throws -> DevicesResult {
        try DevicesResult(await client.send(Ops.devices))
    }

    // MARK: 登录

    public func loginRequestCode(server: String, email: String, ca: String? = nil) async throws -> LoginRequestCodeResult {
        try LoginRequestCodeResult(await client.send(
            Ops.loginRequestCode,
            params: Contract.loginRequestCodeParams(server: server, email: email, ca: ca),
        ))
    }

    public func loginVerify(code: String, email: String, name: String) async throws -> LoginVerifyResult {
        try LoginVerifyResult(await client.send(
            Ops.loginVerify,
            params: Contract.loginVerifyParams(code: code, email: email, name: name),
        ))
    }

    public func loginLogout() async throws {
        _ = try await client.send(Ops.loginLogout).asObject()
    }

    // MARK: 浏览

    public func list(device: String, path: String) async throws -> ListResult {
        try ListResult(await client.send(
            Ops.list,
            params: Contract.listParams(device: device, path: path),
        ))
    }

    public func stat(device: String, path: String) async throws -> StatResult {
        try StatResult(await client.send(
            Ops.stat,
            params: Contract.statParams(device: device, path: path),
        ))
    }

    // MARK: 传输

    public func transferStartUpload(
        files: [String], to: String, dir: String,
        skip: [String] = [], overwrite: [String] = [], cut: Bool = false,
    ) async throws -> TransferStartResult {
        try TransferStartResult(await client.send(
            Ops.transferStart,
            params: Contract.transferStartUploadParams(
                files: files, to: to, dir: dir, skip: skip, overwrite: overwrite, cut: cut,
            ),
        ))
    }

    public func transferStartUploadBridge(
        items: [BridgeItem], to: String, dir: String,
        skip: [String] = [], overwrite: [String] = [], cut: Bool = false,
    ) async throws -> TransferStartResult {
        try TransferStartResult(await client.send(
            Ops.transferStart,
            params: Contract.transferStartUploadBridgeParams(
                items: items, to: to, dir: dir, skip: skip, overwrite: overwrite, cut: cut,
            ),
        ))
    }

    public func transferStartDownload(
        device: String, paths: [String], dir: String,
        skip: [String] = [], overwrite: [String] = [], cut: Bool = false,
    ) async throws -> TransferStartResult {
        try TransferStartResult(await client.send(
            Ops.transferStart,
            params: Contract.transferStartDownloadParams(
                device: device, paths: paths, dir: dir, skip: skip, overwrite: overwrite, cut: cut,
            ),
        ))
    }

    public func transferList() async throws -> TransferListResult {
        try TransferListResult(await client.send(Ops.transferList))
    }

    public func transferPause(id: String) async throws {
        _ = try await client.send(Ops.transferPause, params: Contract.transferIdParams(id: id)).asObject()
    }

    public func transferResume(id: String) async throws {
        _ = try await client.send(Ops.transferResume, params: Contract.transferIdParams(id: id)).asObject()
    }

    public func transferCancel(id: String) async throws {
        _ = try await client.send(Ops.transferCancel, params: Contract.transferIdParams(id: id)).asObject()
    }

    public func transferWait(id: String) async throws -> TransferWaitResult {
        try TransferWaitResult(await client.send(Ops.transferWait, params: Contract.transferIdParams(id: id)))
    }

    public func transferReinitiate(id: String) async throws {
        _ = try await client.send(Ops.transferReinitiate, params: Contract.transferIdParams(id: id)).asObject()
    }

    public func transferClear(id: String) async throws {
        _ = try await client.send(Ops.transferClear, params: Contract.transferIdParams(id: id)).asObject()
    }
}
