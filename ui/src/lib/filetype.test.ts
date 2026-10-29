import { describe, expect, it } from "vitest";

import { fileTypeTintKey, fileTypeName } from "./filetype";

describe("fileTypeName", () => {
  it("策展映射命中(扩展名大小写不敏感,取最后一段扩展名)", () => {
    expect(fileTypeName("report.pdf", false)).toBe("PDF 文档");
    expect(fileTypeName("Report.PDF", false)).toBe("PDF 文档");
    expect(fileTypeName("sheet.xlsx", false)).toBe("Excel 工作表");
    expect(fileTypeName("数据.csv", false)).toBe("Excel 工作表");
    expect(fileTypeName("doc.docx", false)).toBe("Word 文档");
    expect(fileTypeName("movie.mp4", false)).toBe("视频");
    expect(fileTypeName("song.flac", false)).toBe("音频");
    expect(fileTypeName("pic.jpeg", false)).toBe("图片");
    expect(fileTypeName("backup.tar.gz", false)).toBe("压缩包");
    expect(fileTypeName("setup.EXE", false)).toBe("应用程序");
    expect(fileTypeName("notes.md", false)).toBe("文本文档");
  });

  it("未知扩展名 → 「大写扩展名 + 文件」", () => {
    expect(fileTypeName("archive.zzz", false)).toBe("ZZZ 文件");
  });

  it("无扩展名 → 「文件」(无点 / 点文件 / 尾点同口径)", () => {
    expect(fileTypeName("README", false)).toBe("文件");
    expect(fileTypeName(".gitignore", false)).toBe("文件");
    expect(fileTypeName("name.", false)).toBe("文件");
  });

  it("目录恒为「文件夹」(不看名称,有扩展名也一样)", () => {
    expect(fileTypeName("anything", true)).toBe("文件夹");
    expect(fileTypeName("photo.jpg", true)).toBe("文件夹");
  });
});

describe("fileTypeTintKey", () => {
  it("策展类别 → 各自取色键(pdf/xls/vid/exe/txt/img/archive)", () => {
    expect(fileTypeTintKey("a.pdf", false)).toBe("pdf");
    expect(fileTypeTintKey("a.xlsx", false)).toBe("xls");
    expect(fileTypeTintKey("a.mp4", false)).toBe("vid");
    expect(fileTypeTintKey("a.exe", false)).toBe("exe");
    expect(fileTypeTintKey("a.txt", false)).toBe("txt");
    expect(fileTypeTintKey("a.png", false)).toBe("img");
    expect(fileTypeTintKey("a.zip", false)).toBe("archive");
  });

  it("Word/音频无专属角标色 → generic;未知/无扩展名 → generic;目录 → folder", () => {
    expect(fileTypeTintKey("a.docx", false)).toBe("generic");
    expect(fileTypeTintKey("a.mp3", false)).toBe("generic");
    expect(fileTypeTintKey("a.zzz", false)).toBe("generic");
    expect(fileTypeTintKey("README", false)).toBe("generic");
    expect(fileTypeTintKey("d", true)).toBe("folder");
  });
});
