/// 浏览行图标(spec §4.1:统一两枚,不按文件类型区分——用户裁定;mockup
/// browse-final 像素参照:文件夹 = 蓝色双层文件夹,文件 = 问号文档)。

import SwiftUI

struct FileTypeIcon: View {
    let isDir: Bool

    var body: some View {
        if isDir {
            Image(systemName: "folder.fill")
                .font(.system(size: 23))
                .foregroundStyle(Color.accentColor)
                .frame(width: 30)
        } else {
            // 问号文档:doc 线框 + 小问号(文件 App「未知类型」同象)
            Image(systemName: "doc")
                .font(.system(size: 21))
                .foregroundStyle(Color(uiColor: .systemGray))
                .overlay {
                    Text("?")
                        .font(.system(size: 9, weight: .bold))
                        .foregroundStyle(Color(uiColor: .systemGray))
                        .offset(y: 3)
                }
                .frame(width: 30)
        }
    }
}

/// 根列表行图标(快捷目录授权根;同文件夹形)。
struct RootFolderIcon: View {
    var body: some View {
        Image(systemName: "folder.fill")
            .font(.system(size: 23))
            .foregroundStyle(Color.accentColor)
            .frame(width: 30)
    }
}
