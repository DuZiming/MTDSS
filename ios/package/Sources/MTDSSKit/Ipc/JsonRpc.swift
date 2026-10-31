/// MTDSSKit — JSON-RPC 线格式层(NDJSON,一行一帧)。
///
/// 协议事实对照 crates/ipc/src/lib.rs(单一真源):
/// - 请求:  `{"jsonrpc":"2.0","id":N,"method":"...","params":...}`
/// - 响应:  `{"id":N,"result":...}` 或 `{"id":N,"error":{"code":i64,"message":"...","data"?…}}`
/// - 事件:  `{"jsonrpc":"2.0","method":"...","params":...}`(无 id)
/// - 单帧(单行)上限 8 MiB,超限断连。
///
/// ## 规范写出 = serde_json 字节口径(契约关键,ContractGoldenTests 逐字节锁定)
/// daemon 侧 serde_json 默认 BTreeMap 键序 = **UTF-8 字节序**;请求帧必须与之
/// 逐字节一致,故 `JSONValue.canonical` 递归按键排序写出。转义面与 serde_json
/// 对齐:仅 `"`、`\` 与 <0x20 控制字符(`\b\f\n\r\t` 快捷式,其余 `\u00xx`
/// 小写十六进制);非 ASCII(含 CJK)原样直出、不转义 `/`。
///
/// ## 数值以原文保留
/// 解析往返无损;构造侧整型经 `int`/`uint` 生成十进制原文,与 Rust i64 Display
/// 一致。浮点字段不在契约面(全字段整型/字符串/布尔)。`Codable` 一致化仅作
/// 外围互操作之用,线上帧一律走 canonical 写出(注:JSONDecoder 的 Decimal
/// 策略对 "1e3" 等指数原文不保形,契约面无此形态)。

import Foundation

/// JSON 值模型(线协议统一值面;解析与写出零第三方依赖)。
public enum JSONValue: Sendable, Hashable {
    case null
    case bool(Bool)
    /// 数值原文(如 "1"、"1760000000000"、"2.5")。
    case number(String)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])
}

// MARK: - 构造便利 + 类型化访问(与键序无关)

public extension JSONValue {
    /// 整型构造:十进制原文,与 Rust i64/UInt64 Display 一致。
    static func int(_ v: Int64) -> JSONValue { .number(String(v)) }
    static func uint(_ v: UInt64) -> JSONValue { .number(String(v)) }

    var isNull: Bool {
        if case .null = self { return true }
        return false
    }

    /// 对象字段取值;非对象或缺键 → nil(与键序无关)。
    subscript(key: String) -> JSONValue? {
        guard case .object(let obj) = self else { return nil }
        return obj[key]
    }

    /// 必填字段;缺失/非对象即契约违规。
    func req(_ key: String) throws -> JSONValue {
        guard let v = self[key] else {
            throw ContractViolationError("缺少字段: \(key)")
        }
        return v
    }

    func asString() throws -> String {
        guard case .string(let s) = self else {
            throw ContractViolationError("非字符串值: \(canonical)")
        }
        return s
    }

    /// 可空投影:JSON null → nil;其余类型错即违规。
    func asStringOrNull() throws -> String? {
        switch self {
        case .string(let s): return s
        case .null: return nil
        default: throw ContractViolationError("非字符串/null 值: \(canonical)")
        }
    }

    /// 可空字段投影:键缺失 → nil(JSON null 同);存在但非字符串/null → 违规。
    func optString(_ key: String) throws -> String? {
        guard let v = self[key], !v.isNull else { return nil }
        return try v.asStringOrNull()
    }

    func asInt64() throws -> Int64 {
        guard case .number(let raw) = self, let n = Int64(raw) else {
            throw ContractViolationError("非整数值: \(canonical)")
        }
        return n
    }

    func asBool() throws -> Bool {
        guard case .bool(let b) = self else {
            throw ContractViolationError("非布尔值: \(canonical)")
        }
        return b
    }

    func asArray() throws -> [JSONValue] {
        guard case .array(let a) = self else {
            throw ContractViolationError("非数组值: \(canonical)")
        }
        return a
    }

    func asObject() throws -> [String: JSONValue] {
        guard case .object(let o) = self else {
            throw ContractViolationError("非对象值: \(canonical)")
        }
        return o
    }

    /// 规范化写出(serde_json 字节口径;见文件头注)。
    var canonical: String {
        var out = ""
        writeCanonical(into: &out)
        return out
    }

    private func writeCanonical(into out: inout String) {
        switch self {
        case .null: out += "null"
        case .bool(let b): out += b ? "true" : "false"
        case .number(let raw): out += raw
        case .string(let s): writeJSONString(s, into: &out)
        case .array(let items):
            out += "["
            for (i, item) in items.enumerated() {
                if i > 0 { out += "," }
                item.writeCanonical(into: &out)
            }
            out += "]"
        case .object(let obj):
            out += "{"
            // serde_json BTreeMap<Vec<u8>> 键序 = UTF-8 字节序
            let keys = obj.keys.sorted { $0.utf8.lexicographicallyPrecedes($1.utf8) }
            for (i, key) in keys.enumerated() {
                if i > 0 { out += "," }
                writeJSONString(key, into: &out)
                out += ":"
                obj[key]!.writeCanonical(into: &out)
            }
            out += "}"
        }
    }
}

/// serde_json 兼容字符串转义:仅 " \ 与 <0x20 控制;非 ASCII 原样直出。
private func writeJSONString(_ v: String, into out: inout String) {
    out += "\""
    for scalar in v.unicodeScalars {
        switch scalar {
        case "\"": out += "\\\""
        case "\\": out += "\\\\"
        case "\u{08}": out += "\\b"
        case "\u{0C}": out += "\\f"
        case "\n": out += "\\n"
        case "\r": out += "\\r"
        case "\t": out += "\\t"
        default:
            if scalar.value < 0x20 {
                out += String(format: "\\u%04x", scalar.value)
            } else {
                out.unicodeScalars.append(scalar)
            }
        }
    }
    out += "\""
}

// MARK: - 解析(递归下降,严格标准 JSON)

/// JSON 解析失败(非法字面量/结构截断/悬挂代理对等)。
public struct JSONParseError: Error, Sendable, CustomStringConvertible {
    public let message: String
    public init(_ message: String) { self.message = message }
    public var description: String { message }
}

/// 契约违规:应答帧畸形或 result 形状与类型化解析不符。
public struct ContractViolationError: Error, Sendable, CustomStringConvertible {
    public let message: String
    public init(_ message: String) { self.message = message }
    public var description: String { message }
}

/// 标准 JSON 解析(严格口径:注释/尾逗号不支持;悬挂代理对拒绝,镜像 Rust
/// serde_json;索引推进,字节级操作对 UTF-8 多字节原样直通)。
public enum JSONParser {
    public static func parse(_ text: String) throws -> JSONValue {
        var parser = ParserImpl(text: text)
        return try parser.parseRoot()
    }
}

private struct ParserImpl {
    private let bytes: [UInt8]
    private var i = 0

    init(text: String) {
        bytes = Array(text.utf8)
    }

    private var peek: UInt8? { i < bytes.count ? bytes[i] : nil }

    private func err(_ message: String) -> JSONParseError {
        JSONParseError("\(message) @\(i)")
    }

    mutating func parseRoot() throws -> JSONValue {
        skipWS()
        let v = try value()
        skipWS()
        guard i >= bytes.count else { throw err("JSON 尾部多余内容") }
        return v
    }

    private mutating func skipWS() {
        while let b = peek, b == 0x20 || b == 0x09 || b == 0x0A || b == 0x0D { i += 1 }
    }

    private mutating func value() throws -> JSONValue {
        guard let b = peek else { throw err("JSON 意外结束") }
        switch b {
        case UInt8(ascii: "{"): return try object()
        case UInt8(ascii: "["): return try array()
        case UInt8(ascii: "\""): return .string(try stringLiteral())
        case UInt8(ascii: "t"): return try literal("true", .bool(true))
        case UInt8(ascii: "f"): return try literal("false", .bool(false))
        case UInt8(ascii: "n"): return try literal("null", .null)
        default: return try numberLiteral()
        }
    }

    private mutating func literal(_ word: String, _ v: JSONValue) throws -> JSONValue {
        let w = Array(word.utf8)
        guard i + w.count <= bytes.count, Array(bytes[i..<i + w.count]) == w else {
            throw err("非法字面量")
        }
        i += w.count
        return v
    }

    private mutating func object() throws -> JSONValue {
        i += 1 // '{'
        var map = [String: JSONValue]()
        skipWS()
        if peek == UInt8(ascii: "}") {
            i += 1
            return .object(map)
        }
        while true {
            skipWS()
            guard peek == UInt8(ascii: "\"") else { throw err("对象键须为字符串") }
            let key = try stringLiteral()
            skipWS()
            guard peek == UInt8(ascii: ":") else { throw err("缺 ':'") }
            i += 1
            skipWS()
            map[key] = try value()
            skipWS()
            guard let b = peek else { throw err("对象未闭合") }
            switch b {
            case UInt8(ascii: ","):
                i += 1
            case UInt8(ascii: "}"):
                i += 1
                return .object(map)
            default:
                throw err("对象内非法字符 '\(Character(UnicodeScalar(b)))'")
            }
        }
    }

    private mutating func array() throws -> JSONValue {
        i += 1 // '['
        var items = [JSONValue]()
        skipWS()
        if peek == UInt8(ascii: "]") {
            i += 1
            return .array(items)
        }
        while true {
            skipWS()
            items.append(try value())
            skipWS()
            guard let b = peek else { throw err("数组未闭合") }
            switch b {
            case UInt8(ascii: ","):
                i += 1
            case UInt8(ascii: "]"):
                i += 1
                return .array(items)
            default:
                throw err("数组内非法字符 '\(Character(UnicodeScalar(b)))'")
            }
        }
    }

    /// 字符串字面量(调用时 i 指向开引号);\uXXXX 代理对合成标量,悬挂代理拒绝。
    private mutating func stringLiteral() throws -> String {
        i += 1 // '"'
        var buf = [UInt8]()
        while true {
            guard i < bytes.count else { throw err("字符串未闭合") }
            let b = bytes[i]
            switch b {
            case UInt8(ascii: "\""):
                i += 1
                return String(decoding: buf, as: UTF8.self)
            case UInt8(ascii: "\\"):
                i += 1
                guard i < bytes.count else { throw err("转义未闭合") }
                switch bytes[i] {
                case UInt8(ascii: "\""): buf.append(UInt8(ascii: "\""))
                case UInt8(ascii: "\\"): buf.append(UInt8(ascii: "\\"))
                case UInt8(ascii: "/"): buf.append(UInt8(ascii: "/"))
                case UInt8(ascii: "b"): buf.append(0x08)
                case UInt8(ascii: "f"): buf.append(0x0C)
                case UInt8(ascii: "n"): buf.append(0x0A)
                case UInt8(ascii: "r"): buf.append(0x0D)
                case UInt8(ascii: "t"): buf.append(0x09)
                case UInt8(ascii: "u"): try appendUnicodeEscape(into: &buf)
                default: throw err("非法转义 '\\\(Character(UnicodeScalar(bytes[i])))'")
                }
                i += 1
            default:
                guard b >= 0x20 else { throw err("字符串含未转义控制字符") }
                buf.append(b)
                i += 1
            }
        }
    }

    /// 读 \uXXXX 十六进制(调用时 i 指向 'u');返回后 i 指向末位十六进制。
    private mutating func hex4() throws -> UInt32 {
        guard i + 5 <= bytes.count else { throw err("\\u 转义不完整") }
        var v: UInt32 = 0
        for k in (i + 1)...(i + 4) {
            let b = bytes[k]
            let d: UInt32
            switch b {
            case 0x30...0x39: d = UInt32(b - 0x30)
            case 0x61...0x66: d = UInt32(b - 0x61) + 10
            case 0x41...0x46: d = UInt32(b - 0x41) + 10
            default: throw err("非法 \\u 转义")
            }
            v = v << 4 | d
        }
        i += 4
        return v
    }

    private mutating func appendUnicodeEscape(into buf: inout [UInt8]) throws {
        let first = try hex4()
        let scalarValue: UInt32
        if (0xD800...0xDBFF).contains(first) {
            // 高代理:必须紧跟低代理
            guard i + 7 <= bytes.count, bytes[i + 1] == UInt8(ascii: "\\"), bytes[i + 2] == UInt8(ascii: "u") else {
                throw err("悬挂高代理")
            }
            i += 2 // 指向第二个 'u'
            let second = try hex4()
            guard (0xDC00...0xDFFF).contains(second) else { throw err("高代理未配低代理") }
            scalarValue = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
        } else if (0xDC00...0xDFFF).contains(first) {
            throw err("悬挂低代理")
        } else {
            scalarValue = first
        }
        guard let scalar = Unicode.Scalar(scalarValue) else { throw err("非法码点") }
        buf.append(contentsOf: String(scalar).utf8)
    }

    /// 数值字面量:按原文捕获([-]?digits[.digits][[eE][+-]?digits])。
    private mutating func numberLiteral() throws -> JSONValue {
        let start = i
        if peek == UInt8(ascii: "-") { i += 1 }
        guard let b0 = peek, (0x30...0x39).contains(b0) else { throw err("非法数值") }
        while let b = peek, (0x30...0x39).contains(b) { i += 1 }
        if peek == UInt8(ascii: ".") {
            i += 1
            guard let b = peek, (0x30...0x39).contains(b) else { throw err("小数部分缺数字") }
            while let b = peek, (0x30...0x39).contains(b) { i += 1 }
        }
        if let e = peek, e == UInt8(ascii: "e") || e == UInt8(ascii: "E") {
            i += 1
            if let s = peek, s == UInt8(ascii: "+") || s == UInt8(ascii: "-") { i += 1 }
            guard let d = peek, (0x30...0x39).contains(d) else { throw err("指数部分缺数字") }
            while let d = peek, (0x30...0x39).contains(d) { i += 1 }
        }
        return .number(String(decoding: bytes[start..<i], as: UTF8.self))
    }
}

// MARK: - Codable(外围互操作用;线上帧走 canonical,见文件头注)

extension JSONValue: Codable {
    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() {
            self = .null
        } else if let b = try? c.decode(Bool.self) {
            self = .bool(b)
        } else if let n = try? c.decode(Decimal.self) {
            self = .number(String(describing: n))
        } else if let s = try? c.decode(String.self) {
            self = .string(s)
        } else if let a = try? c.decode([JSONValue].self) {
            self = .array(a)
        } else if let o = try? c.decode([String: JSONValue].self) {
            self = .object(o)
        } else {
            throw DecodingError.dataCorruptedError(in: c, debugDescription: "不支持的 JSON 值")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .bool(let b): try c.encode(b)
        case .number(let raw):
            if let i = Int64(raw) {
                try c.encode(i)
            } else if let d = Double(raw) {
                try c.encode(d)
            } else {
                throw EncodingError.invalidValue(
                    raw,
                    .init(codingPath: encoder.codingPath, debugDescription: "非法数值原文: \(raw)"),
                )
            }
        case .string(let s): try c.encode(s)
        case .array(let a): try c.encode(a)
        case .object(let o): try c.encode(o)
        }
    }
}

// MARK: - 帧构造与应答信封

/// 请求帧构造:`{"jsonrpc":"2.0","id":N,"method":…,"params":…}` + '\n'。
/// 键序 = serde_json 字节序(canonical);首调 id=1 与 golden 逐字节锚对齐;
/// params 缺省按空对象出线(镜像 Kotlin)。
public func buildRequestLine(id: UInt64, method: String, params: JSONValue?) -> Data {
    let frame = JSONValue.object([
        "jsonrpc": .string("2.0"),
        "id": .uint(id),
        "method": .string(method),
        "params": params ?? .object([:]),
    ])
    return Data((frame.canonical + "\n").utf8)
}

/// RPC/传输层错误:code(-32000 = 传输层,与 Rust/Kotlin IpcClient 同码;
/// 其余为业务码),kind = 服务端 `error.data.kind` 稳定码
/// (NOT_FOUND/PERMISSION_DENIED/…,可空)。
public struct RpcError: Error, Sendable, Equatable, CustomStringConvertible {
    public let code: Int64
    public let message: String
    public let kind: String?

    public init(code: Int64, message: String, kind: String? = nil) {
        self.code = code
        self.message = message
        self.kind = kind
    }

    /// 传输层错误码(-32000 = 传输/断连/超限/写入失败)。
    public static let transportCode: Int64 = -32000

    public static func transport(_ message: String) -> RpcError {
        RpcError(code: transportCode, message: message)
    }

    public var description: String {
        "RpcError(code: \(code), message: \(message), kind: \(kind ?? "nil"))"
    }
}

/// JSON-RPC 信封分拣(镜像 crates/ipc lib.rs 头注口径)。
public enum JSONRpc {

    /// 应答信封:`{"id":N,"result":…}` 或 `{"id":N,"error":…}` → 结果/错误。
    /// 畸形错误帧按传输层错误口径;无 error 键即成功(缺 result → null)。
    public static func parseResponse(_ frame: JSONValue) -> Result<JSONValue, RpcError> {
        guard let err = frame["error"] else {
            return .success(frame["result"] ?? .null)
        }
        guard case .object(let e) = err else {
            return .failure(.transport("畸形错误帧: error 非对象"))
        }
        guard case .number(let raw)? = e["code"], let code = Int64(raw) else {
            return .failure(.transport("畸形错误帧: code 缺失或非整数"))
        }
        var message = ""
        if case .string(let s)? = e["message"] { message = s }
        var kind: String?
        if case .object(let data)? = e["data"], case .string(let k)? = data["kind"] {
            kind = k
        }
        return .failure(RpcError(code: code, message: message, kind: kind))
    }
}
