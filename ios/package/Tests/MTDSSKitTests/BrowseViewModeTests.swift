import XCTest
import MTDSSKit

/// 浏览视图形态记忆单测(V4-1 T24;镜像 android BrowseViewModeTest):
/// 缺省列表 / 档面垃圾值不猜 / 同 store 实例往返(档值不冻结在构造——
/// 安卓 fix round 1 Important #1 同课:load() 每次现查档)/ 键值字面。
final class BrowseViewModeTests: XCTestCase {

    /// 独立 suite(测试互不串档)。
    private func makeDefaults() -> UserDefaults {
        let name = "BrowseViewModeTests-" + UUID().uuidString
        let d = UserDefaults(suiteName: name)!
        d.removePersistentDomain(forName: name)
        return d
    }

    func testKeyLiteralAndDefaultMode() {
        XCTAssertEqual(BrowseViewModeStore.key, "browse_view_mode", "档键 brief 钉死逐字")
        let store = BrowseViewModeStore(defaults: makeDefaults())
        XCTAssertEqual(store.load(), .list, "档面缺席 = 缺省列表(钦定 mobile-browse 列表默认)")
    }

    func testEnumKeysVerbatim() {
        XCTAssertEqual(BrowseViewMode.list.rawValue, "list")
        XCTAssertEqual(BrowseViewMode.tiles.rawValue, "tiles")
    }

    func testGarbageValueFallsBackToListWithoutGuessing() {
        let defaults = makeDefaults()
        for garbage in ["grid", "LIST", "", "tiles ", "tiles\t"] {
            defaults.set(garbage, forKey: BrowseViewModeStore.key)
            let store = BrowseViewModeStore(defaults: defaults)
            XCTAssertEqual(store.load(), .list, "垃圾值「\(garbage)」不猜,归缺省列表")
        }
    }

    func testSameStoreInstanceRoundTripIsNotFrozen() {
        // 同一 store 实例 save → load 即翻(安卓冻结面先案:档值不得在构造
        // 读一次冻结;新组合的浏览屏现查档才成立)。
        let defaults = makeDefaults()
        let store = BrowseViewModeStore(defaults: defaults)
        XCTAssertEqual(store.load(), .list)
        store.save(.tiles)
        XCTAssertEqual(store.load(), .tiles, "同实例 save 后 load 现查档即翻")
        store.save(.list)
        XCTAssertEqual(store.load(), .list)
    }

    func testCrossInstancePersistence() {
        let name = "BrowseViewModeTests-" + UUID().uuidString
        let defaults = UserDefaults(suiteName: name)!
        defaults.removePersistentDomain(forName: name)
        BrowseViewModeStore(defaults: defaults).save(.tiles)
        XCTAssertEqual(BrowseViewModeStore(defaults: defaults).load(), .tiles, "跨实例持久化成立")
    }

    func testOtherFlipsBothWays() {
        XCTAssertEqual(BrowseViewMode.list.other, .tiles)
        XCTAssertEqual(BrowseViewMode.tiles.other, .list)
    }
}
