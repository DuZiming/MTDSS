/// parseAddressInput 单测(P4b Task 4,spec §7 解析规则钉死):设备前缀
/// 命中优先级(id 精确 > 名唯一大小写不敏感 > 同名歧义)/未命中、裸设备
/// 名进设备根(含尾随分隔符形态)、混合分隔符按目标设备平台归一(双向)、
/// 盘根尾斜杠特例(裸盘符补回分隔符成盘根,终审修复波)、相对路径与当前
/// 目录拼接(含卷列表位)、设备层无前缀输入一律
/// needDeviceOrAbsolute、空输入语义、带空格路径、跨平台绝对形归一;
/// 快捷命名空间语法(V3-3 返工 §15.5):直达 fav / favdir、多成员
/// favAmbiguous 拒绝、名未命中落设备解析、同名设备碰撞快捷优先、
/// favdirAddressDraft 预填往返。
import { describe, expect, it } from "vitest";

import { favdirAddressDraft, parseAddressInput } from "./address";

/// 测试设备表:覆盖 windows / linux 平台、大小写混合英文名、null 名、
/// 同名两台(歧义面)。
const DEVICES = [
  { device_id: "id-win", name: "公司电脑", platform: "windows" },
  { device_id: "id-linux", name: "家里的Linux主机", platform: "linux" },
  { device_id: "id-nas", name: "HomeNAS", platform: "linux" },
  { device_id: "id-anon", name: null, platform: "linux" },
  { device_id: "id-dup-a", name: "同名机", platform: "linux" },
  { device_id: "id-dup-b", name: "同名机", platform: "windows" },
];

const WIN = { device: "id-win", path: "D:\\工作文档" };
const POS = { device: "id-linux", path: "/home" };

/// 快捷清单(§15.5):恰一成员(win/posix 各一)、多成员、空成员。
const FAVS = [
  { id: "f-win", name: "项目", members: [{ device: "id-win", path: "D:\\work\\proj" }] },
  { id: "f-posix", name: "posix书", members: [{ device: "id-linux", path: "/home/kk" }] },
  {
    id: "f-multi",
    name: "多成员",
    members: [
      { device: "id-win", path: "C:\\a" },
      { device: "id-linux", path: "/b" },
    ],
  },
  { id: "f-empty", name: "空快捷", members: [] },
];

describe("设备前缀形(id/名解析)", () => {
  it("设备 id 精确命中,且优先于同名(他人名字恰为另一台 id)", () => {
    const devs = [
      { device_id: "id-a", name: "id-b", platform: "windows" },
      { device_id: "id-b", name: "其他", platform: "windows" },
    ];
    expect(parseAddressInput("\\id-b\\D:\\p", devs, null)).toEqual({
      kind: "nav",
      device: "id-b",
      path: "D:\\p",
    });
  });

  it("设备名唯一命中(大小写不敏感)", () => {
    expect(parseAddressInput("\\homenas/data", DEVICES, null)).toEqual({
      kind: "nav",
      device: "id-nas",
      path: "/data",
    });
    expect(parseAddressInput("\\HomeNAS/DATA", DEVICES, null)).toEqual({
      kind: "nav",
      device: "id-nas",
      path: "/DATA",
    });
  });

  it("多台同名 → deviceAmbiguous(带候选名)", () => {
    expect(parseAddressInput("\\同名机\\docs", DEVICES, WIN)).toEqual({
      kind: "error",
      code: "deviceAmbiguous",
      name: "同名机",
    });
  });

  it("无命中 → deviceNotFound(带候选名)", () => {
    expect(parseAddressInput("\\不存在的机器\\D:\\x", DEVICES, WIN)).toEqual({
      kind: "error",
      code: "deviceNotFound",
      name: "不存在的机器",
    });
  });

  it("UNC 双前导反斜杠形同样按设备前缀解析", () => {
    expect(parseAddressInput("\\\\id-win\\D:\\docs", DEVICES, null)).toEqual({
      kind: "nav",
      device: "id-win",
      path: "D:\\docs",
    });
  });
});

describe("盘根尾斜杠特例(windows)", () => {
  it("\\设备\\D:\\ 剥尾分隔符后恰剩裸盘符 → 补回成规范盘根", () => {
    expect(parseAddressInput("\\公司电脑\\D:\\", DEVICES, null)).toEqual({
      kind: "nav",
      device: "id-win",
      path: "D:\\",
    });
  });

  it("裸盘符形 \\设备\\D: 同样归一盘根;posix 设备面维持现状(不补)", () => {
    expect(parseAddressInput("\\公司电脑\\D:", DEVICES, null)).toEqual({
      kind: "nav",
      device: "id-win",
      path: "D:\\",
    });
    expect(parseAddressInput("\\HomeNAS\\D:\\", DEVICES, null)).toEqual({
      kind: "nav",
      device: "id-nas",
      path: "D:",
    });
  });
});

describe("裸设备名 = 进设备根", () => {
  it("裸 \\设备名 → deviceRoot", () => {
    expect(parseAddressInput("\\公司电脑", DEVICES, null)).toEqual({
      kind: "deviceRoot",
      device: "id-win",
    });
  });

  it("尾随分隔符(空余段)同样 → deviceRoot", () => {
    expect(parseAddressInput("\\公司电脑\\", DEVICES, null)).toEqual({
      kind: "deviceRoot",
      device: "id-win",
    });
    expect(parseAddressInput("\\\\id-linux\\", DEVICES, null)).toEqual({
      kind: "deviceRoot",
      device: "id-linux",
    });
  });

  it("裸未命中 → deviceNotFound;裸同名 → deviceAmbiguous", () => {
    expect(parseAddressInput("\\幽灵机", DEVICES, WIN)).toEqual({
      kind: "error",
      code: "deviceNotFound",
      name: "幽灵机",
    });
    expect(parseAddressInput("\\同名机", DEVICES, WIN)).toEqual({
      kind: "error",
      code: "deviceAmbiguous",
      name: "同名机",
    });
  });
});

describe("分隔符归一(按目标设备平台,双向)", () => {
  it("目标 windows:正斜杠余段归一为反斜杠;路径含空格原样保留", () => {
    expect(
      parseAddressInput("\\公司电脑/D:\\工作 文档/项目 源码", DEVICES, null),
    ).toEqual({
      kind: "nav",
      device: "id-win",
      path: "D:\\工作 文档\\项目 源码",
    });
  });

  it("目标 posix:反斜杠余段归一为正斜杠,并补根前缀成绝对(mockup 例)", () => {
    expect(parseAddressInput("\\家里的Linux主机\\home\\kk", DEVICES, WIN)).toEqual(
      {
        kind: "nav",
        device: "id-linux",
        path: "/home/kk",
      },
    );
  });
});

describe("无前缀形(当前设备路径)", () => {
  it("相对输入与当前目录拼接(windows)", () => {
    expect(parseAddressInput("项目 源码", DEVICES, WIN)).toEqual({
      kind: "nav",
      device: "id-win",
      path: "D:\\工作文档\\项目 源码",
    });
    expect(parseAddressInput("子目录/内层", DEVICES, WIN)).toEqual({
      kind: "nav",
      device: "id-win",
      path: "D:\\工作文档\\子目录\\内层",
    });
  });

  it("相对输入与当前目录拼接(posix)", () => {
    expect(parseAddressInput("kk/data", DEVICES, POS)).toEqual({
      kind: "nav",
      device: "id-linux",
      path: "/home/kk/data",
    });
  });

  it("卷列表位(path=\"\")拼接 = 输入本身;posix 根位拼出根前缀", () => {
    expect(parseAddressInput("docs", DEVICES, { device: "id-win", path: "" })).toEqual(
      { kind: "nav", device: "id-win", path: "docs" },
    );
    expect(parseAddressInput("tmp", DEVICES, { device: "id-linux", path: "/" })).toEqual(
      { kind: "nav", device: "id-linux", path: "/tmp" },
    );
  });

  it("POSIX 绝对形输入落在 windows 设备 → 归一为反斜杠(不拼接)", () => {
    expect(parseAddressInput("/var/log", DEVICES, WIN)).toEqual({
      kind: "nav",
      device: "id-win",
      path: "\\var\\log",
    });
  });

  it("windows 盘符路径落在 posix 设备 → 盘符为绝对形,归一为正斜杠", () => {
    expect(parseAddressInput("C:\\Users\\me", DEVICES, POS)).toEqual({
      kind: "nav",
      device: "id-linux",
      path: "C:/Users/me",
    });
  });

  it("本平台绝对形直接跳转不拼接", () => {
    expect(parseAddressInput("E:\\data", DEVICES, WIN)).toEqual({
      kind: "nav",
      device: "id-win",
      path: "E:\\data",
    });
    expect(parseAddressInput("/etc", DEVICES, POS)).toEqual({
      kind: "nav",
      device: "id-linux",
      path: "/etc",
    });
  });

  it("设备层(current=null)无前缀输入一律 needDeviceOrAbsolute", () => {
    expect(parseAddressInput("docs", DEVICES, null)).toEqual({
      kind: "error",
      code: "needDeviceOrAbsolute",
    });
    expect(parseAddressInput("D:\\docs", DEVICES, null)).toEqual({
      kind: "error",
      code: "needDeviceOrAbsolute",
    });
    expect(parseAddressInput("/home/kk", DEVICES, null)).toEqual({
      kind: "error",
      code: "needDeviceOrAbsolute",
    });
  });
});

describe("空输入", () => {
  it("空/纯空白:有当前位 = 原位(调用方 App 视为取消,不发导航)", () => {
    expect(parseAddressInput("", DEVICES, WIN)).toEqual({
      kind: "nav",
      device: "id-win",
      path: "D:\\工作文档",
    });
    expect(parseAddressInput("   ", DEVICES, POS)).toEqual({
      kind: "nav",
      device: "id-linux",
      path: "/home",
    });
  });

  it("空/纯空白:设备层 = needDeviceOrAbsolute", () => {
    expect(parseAddressInput("", DEVICES, null)).toEqual({
      kind: "error",
      code: "needDeviceOrAbsolute",
    });
    expect(parseAddressInput(" \t ", DEVICES, null)).toEqual({
      kind: "error",
      code: "needDeviceOrAbsolute",
    });
  });
});

describe("快捷命名空间语法(V3-3 返工 §15.5)", () => {
  it("恰一成员 + 子路径 → favdir(成员 base + 命名空间 rel;前导反斜杠可有可无)", () => {
    expect(parseAddressInput("\\快捷目录\\项目\\src\\深层", DEVICES, null, FAVS)).toEqual({
      kind: "favdir",
      favId: "f-win",
      device: "id-win",
      base: "D:\\work\\proj",
      rel: "src\\深层",
    });
    expect(parseAddressInput("快捷目录\\项目\\src", DEVICES, null, FAVS)).toEqual({
      kind: "favdir",
      favId: "f-win",
      device: "id-win",
      base: "D:\\work\\proj",
      rel: "src",
    });
  });

  it("子路径尾随分隔符吸收(与路径分解纪律同源)", () => {
    expect(parseAddressInput("\\快捷目录\\项目\\src\\", DEVICES, null, FAVS)).toEqual({
      kind: "favdir",
      favId: "f-win",
      device: "id-win",
      base: "D:\\work\\proj",
      rel: "src",
    });
  });

  it("posix 成员:子路径段归一正斜杠(双向分隔符同认,镜像形 1 归一纪律)", () => {
    expect(parseAddressInput("\\快捷目录\\posix书\\docs\\笔记", DEVICES, null, FAVS)).toEqual({
      kind: "favdir",
      favId: "f-posix",
      device: "id-linux",
      base: "/home/kk",
      rel: "docs/笔记",
    });
    expect(parseAddressInput("\\快捷目录\\posix书/docs", DEVICES, null, FAVS)).toEqual({
      kind: "favdir",
      favId: "f-posix",
      device: "id-linux",
      base: "/home/kk",
      rel: "docs",
    });
  });

  it("无子路径 → fav 混合视图(多成员/空成员快捷同样可达)", () => {
    expect(parseAddressInput("快捷目录\\项目", DEVICES, null, FAVS)).toEqual({
      kind: "fav",
      favId: "f-win",
    });
    expect(parseAddressInput("\\快捷目录\\多成员", DEVICES, null, FAVS)).toEqual({
      kind: "fav",
      favId: "f-multi",
    });
    expect(parseAddressInput("\\快捷目录\\空快捷", DEVICES, null, FAVS)).toEqual({
      kind: "fav",
      favId: "f-empty",
    });
  });

  it("多成员 + 子路径 → favAmbiguous 拒绝(带快捷名;空成员同拒)", () => {
    expect(parseAddressInput("\\快捷目录\\多成员\\a", DEVICES, null, FAVS)).toEqual({
      kind: "error",
      code: "favAmbiguous",
      name: "多成员",
    });
    expect(parseAddressInput("快捷目录\\空快捷\\x", DEVICES, null, FAVS)).toEqual({
      kind: "error",
      code: "favAmbiguous",
      name: "空快捷",
    });
  });

  it("快捷名精确匹配:大小写不同/名不存在 → 落设备解析(deviceNotFound 既有约定)", () => {
    expect(parseAddressInput("\\快捷目录\\项目X\\a", DEVICES, null, FAVS)).toEqual({
      kind: "error",
      code: "deviceNotFound",
      name: "快捷目录",
    });
    expect(parseAddressInput("\\快捷目录\\POSIX书\\a", DEVICES, null, FAVS)).toEqual({
      kind: "error",
      code: "deviceNotFound",
      name: "快捷目录",
    });
  });

  it("快捷名单段缺省(裸 \\快捷目录)→ 落设备解析(裸设备名形)", () => {
    expect(parseAddressInput("\\快捷目录", DEVICES, null, FAVS)).toEqual({
      kind: "error",
      code: "deviceNotFound",
      name: "快捷目录",
    });
  });

  it("清单缺省(旧调用形态)→ 快捷语法不参与,零回归", () => {
    expect(parseAddressInput("\\快捷目录\\项目\\src", DEVICES, null)).toEqual({
      kind: "error",
      code: "deviceNotFound",
      name: "快捷目录",
    });
  });

  it("设备名恰为「快捷目录」碰撞(裁定:快捷语法优先,设备分支兜底)", () => {
    const devs = [
      ...DEVICES,
      { device_id: "id-ns", name: "快捷目录", platform: "windows" },
    ];
    // 快捷名命中 → favdir,设备分支不消费
    expect(parseAddressInput("\\快捷目录\\项目\\src", devs, null, FAVS)).toEqual({
      kind: "favdir",
      favId: "f-win",
      device: "id-win",
      base: "D:\\work\\proj",
      rel: "src",
    });
    // 快捷名未命中 → 落设备解析:同名设备照常可达
    expect(parseAddressInput("\\快捷目录", devs, null, FAVS)).toEqual({
      kind: "deviceRoot",
      device: "id-ns",
    });
    expect(parseAddressInput("\\快捷目录\\D:\\docs", devs, null, FAVS)).toEqual({
      kind: "nav",
      device: "id-ns",
      path: "D:\\docs",
    });
  });

  it("favdirAddressDraft 预填与解析往返(favdir 位原位回车)", () => {
    const draft = favdirAddressDraft("项目", ["src", "深层"]);
    expect(draft).toBe("快捷目录\\项目\\src\\深层");
    expect(parseAddressInput(draft, DEVICES, null, FAVS)).toEqual({
      kind: "favdir",
      favId: "f-win",
      device: "id-win",
      base: "D:\\work\\proj",
      rel: "src\\深层",
    });
    // 成员根(无 rel 段):两段形往返落 fav 混合视图(§15.5 无子路径语义)
    const rootDraft = favdirAddressDraft("项目", []);
    expect(rootDraft).toBe("快捷目录\\项目");
    expect(parseAddressInput(rootDraft, DEVICES, null, FAVS)).toEqual({
      kind: "fav",
      favId: "f-win",
    });
  });
});
