
## r59 安卓登录页三修(2026-11-27 末十一;APK Synshu-1.5.0-r59-debug 2c7b46bc)

- **#1 徽记/字标居中**:内容父列无水平居中,裸子件靠左——徽记+SYNSHU
  包 fillMaxWidth 居中块(表单通栏不收缩)。
- **#2「请填写服务器地址与邮箱」根修**:UiState.server 空串缺省 + 全新
  安装无 prefs 预填 → 恒落本地校验拒;iOS P4b r2f 的默认域名修安卓漏了
  ——DEFAULT_SERVER=https://www.mtdss.top 内置缺省,prefs 过往值仍覆盖
  (dev 口;高级选项撤后自定义服务器的唯一进入面)。
- **#3 高级选项全撤**(全设备类型):折叠链接/服务器输入框/CA 占位三件
  撤;CA 演练钩子(dev 服务器回显验证码)留代码面。
- 附:LoginViewModel 测试两枚随新默认(空邮箱才落本地校验 + 只填邮箱即
  触 RPC);PreviewBoot dispatch 断言随 r54 window 面(陈旧锚补)。
**部署**:APK `Synshu-1.5.0-r59-debug.apk`(**2c7b46bc**…;Kotlin 单测
544/544)。

## r60 安卓体验轮(2026-11-27 末十二;APK Synshu-1.5.0-r60-debug 2ad95061)

- **#1 edge-to-edge 快修**:新设备上 tab 壳叠乱根因 = 安卓 15+ 对
  targetSdk 35+ 强制全面屏而 App 零 insets 避让——主题四变体
  (values/night/v35/night-v35)+ windowOptOutEdgeToEdgeEnforcement=true
  退出强制,恢复经典系统代避让(MIX3 同象);insets 正修挂后续票。
- **#2 图标全量换装**:官方 material-icons-extended(BOM 管)Rounded
  族——中心图标表 AppIcons 语义命名;选中态 = **同字形变色 + 胶囊底**
  (iOS tab 范式),fill/stroke 两态跳变根除;扫掉手搓 MockGlyph 路径面
  ~36 处(tab 四钮/文件类型/选择圈/底栏动作/顶栏箭头/扫码/齿轮/占位
  行/设置/设备选择/预览返回与全屏);文档图标 = Material Description
  单图标(双路径+问号叠层随撤)。
- **#3 预览滑切(iOS 对齐)**:图片 = zoom 查看器统一手势面(单指∧未
  放大 = 横滑切图消费阻断变换;双指或放大 = 缩放/平移)——自研
  awaitEachGesture 替 detectTransformGestures;视频/音频 = 容器横滑
  (72f 阈,consume 防串控制器);‹› 箭头整件撤,页点保留;
  PreviewGallery 增 video/audio 兄弟集(环绕步进)。
**部署**:APK `Synshu-1.5.0-r60-debug.apk`(**2ad95061**…;Kotlin 全测
绿)。**过程教训:python 字串锚批量替换 Kotlin 三次滑锚(花括号配平/
函数边界/import 前缀)——多行结构改动必须 Edit 工具逐处,python 只做
单行原子替换。**
