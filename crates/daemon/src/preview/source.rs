//! FileSource 抽象(V4-1 T3):预览按需读的源面。
//!
//! - [`FileSource`]:可 seek 读面的最小接口(size/read_at);实现须
//!   Send+Sync(消费面在 spawn_blocking 阻塞池上逐段读)。
//! - [`FsFileSource`]:桌面主路径 = std::fs 直开(可 seek)。
//! - [`ExportFileSource`]:手机不可 seek 时的回退实现——复用既有
//!   serve_export/photo_export 导出件导轨(App 把原始字节落导出临时目录,
//!   引擎对其可 seek 读);生命周期挂源手柄(Drop 尽力清理临时件,仅落
//!   前缀内才删,fail-closed,App 启动清扫兜底);超时预算按大小计算
//!   ([`export_budget`],秒级小件为主)。
//! - [`open_source`]:源装配(brief 接口面 device/path/mtime/size →
//!   `Arc<dyn FileSource>`;桌面 = std::fs;手机桥实现 T8/T10 挂入真桥
//!   信号,device/mtime 即其会话/缓存键原料,本任务不消费)。
//!
//! 本文件只管源面;file_read_range 的流式处理器与路由在 [`super`]。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 预览数据源(可 seek 读面)。
pub(crate) trait FileSource: Send + Sync {
    /// 源字节数(开源时快照;流中不变的定值)。
    fn size(&self) -> u64;

    /// 定点读:至多填满 `buf`,返回实读字节数;`Ok(0)` = EOF(off 落源末
    /// 之外/源已尽)。短读合法(调用方按返回值推进偏移)。
    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<usize>;
}

/// std::fs 主路径(桌面):文件句柄 + 开源时快照的 size。`Mutex<File>`:
/// seek+read 串行化于单源(单流顺序读,无争用面);导出件回退源复用同形。
pub(crate) struct FsFileSource {
    file: Mutex<std::fs::File>,
    size: u64,
}

impl FsFileSource {
    pub(crate) fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = std::fs::File::open(path.as_ref())?;
        let size = file.metadata()?.len();
        Ok(Self {
            file: Mutex::new(file),
            size,
        })
    }
}

impl FileSource for FsFileSource {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        use std::io::{Read as _, Seek as _, SeekFrom};
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(off))?;
        f.read(buf) // 单次 read;0 = EOF(常规文件面 0 只在源末)
    }
}

/// 导出件回退源(手机不可 seek 时):持 App 导出临时件路径的可 seek 读面
/// 与 Drop 清理生命周期。
pub(crate) struct ExportFileSource {
    inner: FsFileSource,
    temp: PathBuf,
    /// Drop 清理面:Some(prefix) = 仅当 temp 落 prefix 内才删(fail-closed
    /// ——越界路径拒删留痕,App 启动清扫兜底,与 `cleanup_serve_temps` 同
    /// 口径);None = 永不删(单测面)。
    cleanup_prefix: Option<PathBuf>,
}

impl ExportFileSource {
    /// 开源:`temp` = 导出件绝对路径(serve_export/photo_export 0x09/0x07
    /// 应答的回传路径);`cleanup_prefix` = 导出临时目录锚(消费面传入
    /// serve-tmp/photo-serve 目录;None = 不挂清理)。
    pub(crate) fn open(temp: PathBuf, cleanup_prefix: Option<PathBuf>) -> io::Result<Self> {
        let inner = FsFileSource::open(&temp)?;
        Ok(Self {
            inner,
            temp,
            cleanup_prefix,
        })
    }
}

impl FileSource for ExportFileSource {
    fn size(&self) -> u64 {
        self.inner.size()
    }

    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read_at(off, buf)
    }
}

impl Drop for ExportFileSource {
    fn drop(&mut self) {
        let Some(prefix) = &self.cleanup_prefix else {
            return;
        };
        if !self.temp.starts_with(prefix) {
            tracing::warn!(
                path = %self.temp.display(),
                "导出件越出临时目录前缀,拒删(App 清扫兜底)"
            );
            return;
        }
        if let Err(e) = std::fs::remove_file(&self.temp) {
            tracing::warn!(
                path = %self.temp.display(),
                %e,
                "导出件清理失败(App 清扫兜底)"
            );
        }
    }
}

/// 源装配(brief 接口面:device,path,mtime,size → `Arc<dyn FileSource>`)。
/// 桌面 = std::fs 主路径;手机桥不可 seek 的回退实现 T8/T10 挂入真桥信号
/// ——`device`/`mtime` 即其会话/缓存键原料(预览缓存以 (path,size) 退化
/// 键,见 proto 注),本任务不消费。失败(io)由调用方映射 `source_offline`。
pub(crate) fn open_source(
    _device: &str,
    path: &str,
    _mtime: i64,
    _size: u64,
) -> io::Result<Arc<dyn FileSource>> {
    Ok(Arc::new(FsFileSource::open(path)?))
}

/// 导出等待预算(按大小计算;秒级小件为主,大件线性放宽,60s 封顶)。
/// 真桥信号 T8/T10 挂入时的导出等待窗原料(既有 serve/photo export 的定
/// 值超时不动,本预算供预览导出臂按件计算)。常量可调。
#[cfg_attr(not(test), allow(dead_code))] // 消费面 = T8/T10 桥信号挂入;T3 单测锚定
pub(crate) fn export_budget(size: u64) -> Duration {
    const BASE: Duration = Duration::from_secs(5);
    const PER_MIB: Duration = Duration::from_secs(1);
    const CAP: Duration = Duration::from_secs(60);
    let extra = PER_MIB.saturating_mul(u32::try_from(size / (1024 * 1024)).unwrap_or(u32::MAX));
    BASE.saturating_add(extra).min(CAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 图样(与 ctrl_e2e 断言面同款函数式图样,逐字节可复算)。
    fn pattern(i: u64) -> u8 {
        (i.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8
    }

    /// FsFileSource:头/中/尾段读 + EOF Ok(0)+ 越端 Ok(0)+ size 快照。
    #[test]
    fn fs_source_segments_and_eof() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.bin");
        let len: u64 = 100_000;
        let data: Vec<u8> = (0..len).map(pattern).collect();
        std::fs::write(&path, &data).unwrap();
        let src = FsFileSource::open(&path).unwrap();
        assert_eq!(src.size(), len, "size = 开源时快照");

        // 头段
        let mut buf = vec![0u8; 1_000];
        let n = src.read_at(0, &mut buf).unwrap();
        assert_eq!(n, 1_000);
        assert_eq!(&buf[..n], &data[..1_000], "头段字节一致");
        // 中段(非对齐偏移)
        let n = src.read_at(73_457, &mut buf).unwrap();
        assert_eq!(n, 1_000);
        assert_eq!(&buf[..n], &data[73_457..74_457], "中段字节一致");
        // 尾段:短读(请求越出源末只给余量)
        let n = src.read_at(len - 10, &mut buf).unwrap();
        assert_eq!(n, 10, "尾段短读恰给余量");
        assert_eq!(&buf[..n], &data[(len - 10) as usize..], "尾段字节一致");
        // EOF / 越端:Ok(0)
        let n = src.read_at(len, &mut buf).unwrap();
        assert_eq!(n, 0, "源末 read_at = EOF Ok(0)");
        let n = src.read_at(len + 5, &mut buf).unwrap();
        assert_eq!(n, 0, "越端 read_at = Ok(0)");
    }

    /// ExportFileSource:读面与 FsFileSource 同形;落前缀内的临时件随
    /// drop 清理(生命周期挂源手柄)。
    #[test]
    fn export_source_reads_and_cleans_up_in_prefix() {
        let home = tempfile::tempdir().unwrap();
        let serve_tmp = home.path().join("serve-tmp");
        std::fs::create_dir_all(&serve_tmp).unwrap();
        let temp = serve_tmp.join("exp.bin");
        let data: Vec<u8> = (0..5_000u64).map(pattern).collect();
        std::fs::write(&temp, &data).unwrap();

        let src = ExportFileSource::open(temp.clone(), Some(serve_tmp.clone())).unwrap();
        assert_eq!(src.size(), 5_000);
        let mut buf = vec![0u8; 2_000];
        let n = src.read_at(3_000, &mut buf).unwrap();
        assert_eq!(n, 2_000);
        assert_eq!(&buf[..n], &data[3_000..5_000], "导出件段读一致");
        drop(src);
        assert!(!temp.exists(), "落前缀内的导出件随 drop 清理");
    }

    /// fail-closed:前缀外的路径 drop **拒删留痕**(与 cleanup_serve_temps
    /// 同口径;单测临时件由 TempDir 收尾)。
    #[test]
    fn export_source_keeps_out_of_prefix_file() {
        let home = tempfile::tempdir().unwrap();
        let serve_tmp = home.path().join("serve-tmp");
        std::fs::create_dir_all(&serve_tmp).unwrap();
        let outside = home.path().join("outside.bin");
        std::fs::write(&outside, b"keep me").unwrap();
        let src = ExportFileSource::open(outside.clone(), Some(serve_tmp)).unwrap();
        assert_eq!(src.size(), 7);
        drop(src);
        assert!(outside.exists(), "前缀外路径拒删留痕(fail-closed)");
        let _ = std::fs::remove_file(&outside);
    }

    /// 无清理锚(None)永不删(单测面)。
    #[test]
    fn export_source_without_prefix_never_cleans() {
        let dir = tempfile::tempdir().unwrap();
        let temp = dir.path().join("a.bin");
        std::fs::write(&temp, b"x").unwrap();
        let src = ExportFileSource::open(temp.clone(), None).unwrap();
        drop(src);
        assert!(temp.exists(), "无清理锚永不删");
    }

    /// open_source 装配面:本机文件即得 FsFileSource;缺失 → Err(调用方
    /// 映射 source_offline)。
    #[test]
    fn open_source_local_fs_and_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, b"hello").unwrap();
        let src = open_source("self", path.to_string_lossy().as_ref(), 0, 5).unwrap();
        assert_eq!(src.size(), 5);
        let mut buf = [0u8; 8];
        let n = src.read_at(0, &mut buf).unwrap();
        assert_eq!(n, 5, "短读恰给源长");
        assert_eq!(&buf[..n], b"hello");
        assert!(open_source(
            "self",
            dir.path().join("缺.bin").to_string_lossy().as_ref(),
            0,
            0
        )
        .is_err());
    }

    /// 导出预算按大小计算(常量锚):秒级小件 = 基窗;每 MiB +1s;60s 封顶。
    #[test]
    fn export_budget_scales_with_size() {
        assert_eq!(export_budget(0), Duration::from_secs(5), "小件 = 基窗");
        assert_eq!(
            export_budget(3 * 1024 * 1024),
            Duration::from_secs(8),
            "3MiB = 基窗+3s"
        );
        assert_eq!(
            export_budget(10 * 1024 * 1024 * 1024),
            Duration::from_secs(60),
            "10GiB 封顶 60s"
        );
    }
}
