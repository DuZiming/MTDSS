import MTDSSKit
import XCTest

/// FavoritesTree 纯投影钉测试(V3-3 返工二 §14.1,Windows favtree.test.ts
/// 镜像):①目录恒前 + 跨源按名交织;②主比并列保持成员添加序(sortEntries
/// 稳定同象)+ 复合行键 device\0path 同路径跨源两行并存;③重复成员容差
/// (memberIndex 区分身份);④离线/失败成员条目缺席(ok/pending 照常并入);
/// ⑤unavailableCount = offline + failed(ok/pending 不计)。
@MainActor
final class FavoritesTreeTests: XCTestCase {

    // MARK: 条目/状态助手(MixedViewModelTests 同源)

    private nonisolated static func dir(_ name: String, path: String) -> DirEntry {
        DirEntry(name: name, path: path, size: 0, mtimeMs: 0, isDir: true, children: nil)
    }

    private nonisolated static func file(_ name: String, path: String) -> DirEntry {
        DirEntry(name: name, path: path, size: 12, mtimeMs: 0, isDir: false, children: nil)
    }

    /// 单成员状态构造器(entries 仅 ok 态承载;其余态恒空,与混合视图落态同形)。
    private nonisolated static func st(
        _ device: String, _ path: String, _ status: MixedViewModel.MemberStatus,
        _ entries: [DirEntry],
    ) -> MixedViewModel.MemberState {
        .init(member: FavMember(device: device, path: path), status: status, entries: entries)
    }

    // MARK: 合并投影(§14.1)

    func testDirsFirstAndCrossSourceInterleaveByName() {
        // 成员序:dev-a 先、dev-b 后;目录组内按名 m < z,文件组跨源交织
        // a.txt(dev-b)在 b.txt(dev-a)前。
        let states = [
            Self.st("dev-a", "C:\\P", .ok, [
                Self.file("b.txt", path: "C:\\P\\b.txt"),
                Self.dir("z", path: "C:\\P\\z"),
            ]),
            Self.st("dev-b", "/srv", .ok, [
                Self.file("a.txt", path: "/srv/a.txt"),
                Self.dir("m", path: "/srv/m"),
            ]),
        ]
        let rows = FavoritesTree.rootRows(states)
        XCTAssertEqual(rows.map(\.entry.name), ["m", "z", "a.txt", "b.txt"])
        XCTAssertEqual(rows.map(\.device), ["dev-b", "dev-a", "dev-b", "dev-a"])
        XCTAssertEqual(FavoritesTree.sort, .name, "树固定排序 = 名称升序(目录恒前由 less 判定)")
    }

    func testTieKeepsMemberOrderWithCompositeKeys() {
        let states = [
            Self.st("dev-a", "C:\\P", .ok, [Self.file("同名.txt", path: "C:\\P\\同名.txt")]),
            Self.st("dev-b", "/srv", .ok, [Self.file("同名.txt", path: "/srv/同名.txt")]),
        ]
        let rows = FavoritesTree.rootRows(states)
        XCTAssertEqual(rows.count, 2, "同路径跨源两行并存不合并")
        XCTAssertEqual(rows[0].key, "dev-a\u{0000}C:\\P\\同名.txt")
        XCTAssertEqual(rows[0].memberIndex, 0, "并列保持成员添加序")
        XCTAssertEqual(rows[1].key, "dev-b\u{0000}/srv/同名.txt")
        XCTAssertEqual(rows[1].memberIndex, 1)
    }

    func testDuplicateMembersKeepDistinctIdentity() {
        // 重复成员容差(清单残项,§13.8 枚举 id 手法沿袭):同 (device,path)
        // 两成员两行,memberIndex 区分——行身份两两必不同。
        let states = [
            Self.st("dev-a", "/x", .ok, [Self.file("f", path: "/x/f")]),
            Self.st("dev-a", "/x", .ok, [Self.file("f", path: "/x/f")]),
        ]
        let rows = FavoritesTree.rootRows(states)
        XCTAssertEqual(rows.count, 2)
        XCTAssertNotEqual(rows[0].memberIndex, rows[1].memberIndex)
        XCTAssertEqual(rows[0].key, rows[1].key, "复合键同形,身份区分归 memberIndex")
    }

    func testOfflineAndFailedEntriesAbsent() {
        let states = [
            Self.st("dev-a", "/p", .ok, [Self.file("a.txt", path: "/p/a.txt")]),
            Self.st("dev-off", "/x", .offline, []),
            Self.st("dev-fail", "/y", .failed, []),
            Self.st("dev-pend", "/z", .pending, []),
        ]
        XCTAssertEqual(
            FavoritesTree.rootRows(states).map(\.device), ["dev-a"],
            "离线/失败成员条目缺席;ok/pending 照常并入")
    }

    // MARK: 注记计数

    func testUnavailableCountsOfflineAndFailedOnly() {
        let states = [
            Self.st("a", "/p", .ok, []),
            Self.st("b", "/x", .offline, []),
            Self.st("c", "/y", .failed, []),
            Self.st("d", "/w", .failed, []),
            Self.st("e", "/z", .pending, []),
        ]
        XCTAssertEqual(FavoritesTree.unavailableCount(states), 3)
        XCTAssertEqual(FavoritesTree.unavailableCount([Self.st("a", "/p", .ok, [])]), 0)
        XCTAssertEqual(FavoritesTree.unavailableCount([]), 0)
    }
}
