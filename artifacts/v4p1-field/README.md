
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
