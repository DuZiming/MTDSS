//! 隐藏收件区（intake）：对账四态 + 记录持久化 + 完成落盘。
//!
//! 手造记录状态：`build_manifest` 于临时源文件得到清单条目，再按
//! `intake.rs` 模块文档中的记录 JSON schema 直接写记录文件 /
//! `.part` 文件，构造 Full / Resume / Skip / 陈旧（哈希不符）各态。

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::protocol::manifest::{build_manifest, CHUNK_SIZE};
use mtdss_core::transport::intake::{self, FilePlan, PartWriter, INTAKE_DIR};
use mtdss_proto::manifest::{ChunkDesc, Manifest};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

// ---------- 测试工具 ----------

/// 伪随机确定性内容的源文件（同名字同尺寸同内容；名字参与种子，
/// 不同名字同尺寸内容不同——"哈希不符"用例依赖这一点）。
fn make_source(dir: &Path, name: &str, size: usize) -> PathBuf {
    let p = dir.join(name);
    let mut v = vec![0u8; size];
    let mut s: u64 = size as u64 | 1;
    for b in name.bytes() {
        s = s.wrapping_mul(0x100000001B3) ^ b as u64;
    }
    for b in v.iter_mut() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        *b = (s & 0xFF) as u8;
    }
    std::fs::write(&p, v).unwrap();
    p
}

/// 用 build_manifest 对临时源文件签出清单（校验过的真实 entry）。
fn manifest_for(src: &Path, rel: &str) -> Manifest {
    let id = DeviceIdentity::generate();
    build_manifest(&[(src.to_path_buf(), rel.to_string())], &id).unwrap()
}

/// 读源文件第 i 块的真实内容（写 PartWriter 用，保证 finalize 整文件哈希可对上）。
fn source_chunk(src: &Path, chunks: &[ChunkDesc], i: usize) -> Vec<u8> {
    let mut f = std::fs::File::open(src).unwrap();
    f.seek(SeekFrom::Start(chunks[i].offset)).unwrap();
    let mut buf = vec![0u8; chunks[i].length as usize];
    f.read_exact(&mut buf).unwrap();
    buf
}

fn bit_set(bitmap: &[u8], i: usize) -> bool {
    bitmap[i / 8] & (1 << (i % 8)) != 0
}

fn set_bit(bitmap: &mut [u8], i: usize) {
    bitmap[i / 8] |= 1 << (i % 8);
}

fn count_set(bitmap: &[u8]) -> usize {
    bitmap.iter().map(|b| b.count_ones() as usize).sum()
}

/// 手写一条记录文件（schema 见 intake.rs 模块文档；故意绕过 PartWriter，
/// 以便构造崩溃残留等任意状态）。
fn write_record(
    out_dir: &Path,
    rel: &str,
    entry: &mtdss_proto::manifest::FileEntry,
    bitmap: &[u8],
    done: bool,
    final_rel: Option<&str>,
) {
    let rec = serde_json::json!({
        "rel": rel,
        "size": entry.size,
        "sha256": hex::encode(&entry.sha256),
        "chunk_count": entry.chunks.len(),
        "bitmap": hex::encode(bitmap),
        "done": done,
        "final_rel": final_rel,
    });
    let rp = intake::record_path(out_dir, rel);
    std::fs::create_dir_all(rp.parent().unwrap()).unwrap();
    std::fs::write(rp, serde_json::to_vec(&rec).unwrap()).unwrap();
}

/// 读回记录位图（hex 解码）；记录不存在时 panic（测试明确需要记录在场）。
fn read_record_bitmap(out_dir: &Path, rel: &str) -> Vec<u8> {
    let bytes = std::fs::read(intake::record_path(out_dir, rel)).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    hex::decode(v["bitmap"].as_str().unwrap()).unwrap()
}

/// 读回记录的 done 与 final_rel。
fn read_done_final(out_dir: &Path, rel: &str) -> (bool, Option<String>) {
    let bytes = std::fs::read(intake::record_path(out_dir, rel)).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (
        v["done"].as_bool().unwrap(),
        v["final_rel"].as_str().map(|s| s.to_string()),
    )
}

fn bitmap_of(n_chunks: usize) -> Vec<u8> {
    vec![0u8; n_chunks.div_ceil(8)]
}

// ---------- 用例 ----------

/// 对账四态：无记录→Full；部分位图+.part→Resume（缺块位=已收位反相）；
/// done+final 落盘且 size 吻合→Skip；哈希不符→Full 且旧记录/.part 被删。
/// 同时校验多文件清单的等长与保序（第二文件恒为 Skip）。
#[test]
fn reconcile_four_states() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();

    let src_a = make_source(src.path(), "a.bin", CHUNK_SIZE * 3);
    let man = manifest_for(&src_a, "a.bin");
    let entry = man.files[0].clone();

    let src_skip = make_source(src.path(), "skip.bin", 100);
    let man_skip = manifest_for(&src_skip, "skip.bin");
    let entry_skip = man_skip.files[0].clone();

    // 双文件清单：保序断言用（files[0]=a.bin 走四态，files[1]=skip.bin 恒 Skip）。
    // 先把 skip.bin 置为终态（done + 终件在场），再开始 a.bin 的四态流转。
    write_record(
        out.path(),
        "skip.bin",
        &entry_skip,
        &bitmap_of(1),
        true,
        Some("skip.bin"),
    );
    std::fs::write(
        out.path().join("skip.bin"),
        std::fs::read(&src_skip).unwrap(),
    )
    .unwrap();

    // 双文件清单
    let two = Manifest {
        version: man.version,
        transfer_id: man.transfer_id.clone(),
        files: vec![entry.clone(), entry_skip.clone()],
        sender_signature: man.sender_signature.clone(),
    };

    // (a) 无记录 → Full；保序：files[1] 已是终态 Skip
    let plans = intake::reconcile(out.path(), &two);
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0], FilePlan::Full);
    assert_eq!(plans[1], FilePlan::Skip, "顺序必须与 man.files 一致");

    // (b) 部分位图 + 匹配 .part → Resume；缺块位图 = 已收位图的反相
    let mut part_bytes = source_chunk(&src_a, &entry.chunks, 0);
    part_bytes.extend_from_slice(&source_chunk(&src_a, &entry.chunks, 2));
    let pp = intake::part_path(out.path(), "a.bin");
    std::fs::create_dir_all(pp.parent().unwrap()).unwrap();
    std::fs::write(&pp, &part_bytes).unwrap();
    let mut bm = bitmap_of(3);
    set_bit(&mut bm, 0);
    set_bit(&mut bm, 2);
    write_record(out.path(), "a.bin", &entry, &bm, false, None);
    let plans = intake::reconcile(out.path(), &two);
    match &plans[0] {
        FilePlan::Resume(miss) => {
            assert_eq!(miss.len(), bm.len());
            assert!(!bit_set(miss, 0), "已收块 0 不得待收");
            assert!(bit_set(miss, 1), "未收块 1 必须待收");
            assert!(!bit_set(miss, 2), "已收块 2 不得待收");
        }
        other => panic!("expect Resume, got {other:?}"),
    }

    // (c) done + final_rel 存在且 size 吻合 → Skip（此时 "a.txt" 不在场）
    let full_bm = {
        let mut b = bitmap_of(3);
        for i in 0..3 {
            set_bit(&mut b, i);
        }
        b
    };
    write_record(out.path(), "a.bin", &entry, &full_bm, true, Some("a.txt"));
    std::fs::write(out.path().join("a.txt"), std::fs::read(&src_a).unwrap()).unwrap();
    let plans = intake::reconcile(out.path(), &two);
    assert_eq!(plans[0], FilePlan::Skip);

    // (d) 哈希不符（同一 rel 换了内容）→ Full，且旧记录与 .part 被删
    let src_b = make_source(src.path(), "b.bin", CHUNK_SIZE * 3);
    let man_b = manifest_for(&src_b, "a.bin");
    let stale = Manifest {
        version: man_b.version,
        transfer_id: man_b.transfer_id.clone(),
        files: vec![man_b.files[0].clone(), entry_skip.clone()],
        sender_signature: man_b.sender_signature.clone(),
    };
    let plans = intake::reconcile(out.path(), &stale);
    assert_eq!(plans[0], FilePlan::Full);
    assert!(
        !intake::record_path(out.path(), "a.bin").exists(),
        "陈旧记录应被删"
    );
    assert!(
        !intake::part_path(out.path(), "a.bin").exists(),
        "陈旧 .part 应被删"
    );
}

/// Skip 依据 final_rel 解析（冲突改名人）：out_dir 只有 "a (1).txt" 而无
/// "a.txt" 时，final_rel=Some("a (1).txt") 仍判 Skip 而非 Full。
#[test]
fn skip_uses_final_rel_after_conflict_rename() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let srcp = make_source(src.path(), "a.txt", 100);
    let man = manifest_for(&srcp, "a.txt");
    let entry = man.files[0].clone();

    write_record(
        out.path(),
        "a.txt",
        &entry,
        &bitmap_of(1),
        true,
        Some("a (1).txt"),
    );
    std::fs::write(out.path().join("a (1).txt"), std::fs::read(&srcp).unwrap()).unwrap();
    assert_eq!(intake::reconcile(out.path(), &man)[0], FilePlan::Skip);

    // 对照：final_rel 指向的名字不在场 → 不得误判 Skip
    write_record(
        out.path(),
        "a.txt",
        &entry,
        &bitmap_of(1),
        true,
        Some("a.txt"),
    );
    assert_eq!(intake::reconcile(out.path(), &man)[0], FilePlan::Full);
}

/// done=true 但 final 文件不存在（改名后被人删掉）→ Full 重新收。
#[test]
fn done_but_final_missing_is_full() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let srcp = make_source(src.path(), "gone.txt", 42);
    let man = manifest_for(&srcp, "gone.txt");
    write_record(
        out.path(),
        "gone.txt",
        &man.files[0],
        &bitmap_of(1),
        true,
        Some("gone.txt"),
    );
    assert_eq!(intake::reconcile(out.path(), &man)[0], FilePlan::Full);
}

/// 记录文件是截断 JSON 垃圾 → 降级 Full，绝不 panic。
#[test]
fn corrupt_record_degrades_to_full() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let srcp = make_source(src.path(), "c.bin", 77);
    let man = manifest_for(&srcp, "c.bin");
    let rp = intake::record_path(out.path(), "c.bin");
    std::fs::create_dir_all(rp.parent().unwrap()).unwrap();
    std::fs::write(&rp, b"{\"rel\": \"c.bin\", \"siz").unwrap();
    assert_eq!(intake::reconcile(out.path(), &man)[0], FilePlan::Full);
}

/// 0 字节文件（chunk_count=0）：open → finalize 直接落盘。
#[tokio::test]
async fn zero_chunk_file_finalizes() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let srcp = make_source(src.path(), "empty.txt", 0);
    let man = manifest_for(&srcp, "empty.txt");
    let entry = man.files[0].clone();
    assert_eq!(entry.size, 0);
    assert!(entry.chunks.is_empty());

    let w = PartWriter::open(out.path(), &entry, false).await.unwrap();
    assert_eq!(w.missing_count(), 0);
    let fin = w.finalize().await.unwrap();
    assert_eq!(fin, out.path().join("empty.txt"));
    assert!(std::fs::read(&fin).unwrap().is_empty());
    assert!(
        !intake::part_path(out.path(), "empty.txt").exists(),
        ".part 应被清理"
    );
    let (done, final_rel) = read_done_final(out.path(), "empty.txt");
    assert!(done);
    assert_eq!(final_rel.as_deref(), Some("empty.txt"));
}

/// 批量落盘语义：写 5 块后未达阈值，记录位图无新位（允许）；
/// 写满第 16 块自动落盘，记录含全部 16 位（无需显式 flush、未 finalize）。
#[tokio::test]
async fn batched_flush_visible() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let srcp = make_source(src.path(), "big.bin", CHUNK_SIZE * 16);
    let man = manifest_for(&srcp, "big.bin");
    let entry = man.files[0].clone();
    assert_eq!(entry.chunks.len(), 16);

    let mut w = PartWriter::open(out.path(), &entry, false).await.unwrap();
    for i in 0..5 {
        let data = source_chunk(&srcp, &entry.chunks, i);
        w.write_chunk(i, entry.chunks[i].offset, &data)
            .await
            .unwrap();
    }
    // 阈值（16 块/2 秒）未达：批量语义下允许无新位；绝不能超过已写块数
    let bm = read_record_bitmap(out.path(), "big.bin");
    assert!(count_set(&bm) < 5, "批量阈值未达，记录不得提前包含新位");

    for i in 5..16 {
        let data = source_chunk(&srcp, &entry.chunks, i);
        w.write_chunk(i, entry.chunks[i].offset, &data)
            .await
            .unwrap();
    }
    // 第 16 块入账即自动落盘：记录文件已含全部 16 位
    let bm = read_record_bitmap(out.path(), "big.bin");
    assert_eq!(count_set(&bm), 16);
    assert_eq!(w.missing_count(), 0);
}

/// 把源文件全部块写进 writer（保证 finalize 整文件哈希可对上）。
async fn write_all_chunks(
    w: &mut PartWriter,
    src: &Path,
    entry: &mtdss_proto::manifest::FileEntry,
) {
    for i in 0..entry.chunks.len() {
        let data = source_chunk(src, &entry.chunks, i);
        w.write_chunk(i, entry.chunks[i].offset, &data)
            .await
            .unwrap();
    }
}

/// finalize：.part 改名至终名、.part 清理、记录 done+final_rel 落盘；
/// 终名冲突时得 "n (1).txt"，final_rel 记改名后的名字。
#[tokio::test]
async fn finalize_renames_and_cleans() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let srcp = make_source(src.path(), "n.txt", CHUNK_SIZE + 7);
    let man = manifest_for(&srcp, "n.txt");
    let entry = man.files[0].clone();
    assert_eq!(entry.chunks.len(), 2);

    // 第一遍：无冲突，落 "n.txt"
    let mut w = PartWriter::open(out.path(), &entry, false).await.unwrap();
    write_all_chunks(&mut w, &srcp, &entry).await;
    let fin = w.finalize().await.unwrap();
    assert_eq!(fin, out.path().join("n.txt"));
    assert_eq!(
        std::fs::read(&fin).unwrap(),
        std::fs::read(&srcp).unwrap(),
        "内容必须一致"
    );
    assert!(
        !intake::part_path(out.path(), "n.txt").exists(),
        ".part 应被清理"
    );
    let (done, final_rel) = read_done_final(out.path(), "n.txt");
    assert!(done);
    assert_eq!(final_rel.as_deref(), Some("n.txt"));

    // 第二遍：同名冲突，应得 "n (1).txt" 且 final_rel 记改名后名字
    let mut w2 = PartWriter::open(out.path(), &entry, false).await.unwrap();
    write_all_chunks(&mut w2, &srcp, &entry).await;
    let fin2 = w2.finalize().await.unwrap();
    assert_eq!(fin2, out.path().join("n (1).txt"));
    assert!(
        !intake::part_path(out.path(), "n.txt").exists(),
        ".part 应被清理"
    );
    let (done2, final_rel2) = read_done_final(out.path(), "n.txt");
    assert!(done2);
    assert_eq!(final_rel2.as_deref(), Some("n (1).txt"));
    assert!(out.path().join("n.txt").exists(), "旧文件不得被覆盖");
    assert_eq!(std::fs::read(&fin2).unwrap(), std::fs::read(&srcp).unwrap());
}

/// Windows：建区后 `.mtdss` 目录带 FILE_ATTRIBUTE_HIDDEN(0x2)。
#[cfg(windows)]
#[tokio::test]
async fn windows_intake_dir_hidden() {
    use std::os::windows::fs::MetadataExt;
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let srcp = make_source(src.path(), "h.txt", 1);
    let man = manifest_for(&srcp, "h.txt");
    let entry = man.files[0].clone();
    // PartWriter::open 会创建收件区；写入唯一块后 finalize
    let mut w = PartWriter::open(out.path(), &entry, false).await.unwrap();
    let data = source_chunk(&srcp, &entry.chunks, 0);
    w.write_chunk(0, entry.chunks[0].offset, &data)
        .await
        .unwrap();
    w.finalize().await.unwrap();
    let meta = std::fs::metadata(out.path().join(INTAKE_DIR)).unwrap();
    assert_ne!(
        meta.file_attributes() & 0x2,
        0,
        "intake 目录必须带 FILE_ATTRIBUTE_HIDDEN"
    );
}

/// 填充位被污染的记录（chunk_count=3 但位图字节 0xFF，即 3 个已收位之外
/// 的 5 个填充位也被置 1 的磁盘损坏形态）：载入时必须掩掉填充位——
/// reconcile 与 open(resume=true) 均不 panic，missing_count 精确为 0
///（不得因「已收」计数虚高导致 usize 下溢回绕）。
#[tokio::test]
async fn padded_bitmap_record_is_masked_not_underflow() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let srcp = make_source(src.path(), "pad.bin", CHUNK_SIZE * 3);
    let man = manifest_for(&srcp, "pad.bin");
    let entry = man.files[0].clone();
    assert_eq!(entry.chunks.len(), 3);

    let pp = intake::part_path(out.path(), "pad.bin");
    std::fs::create_dir_all(pp.parent().unwrap()).unwrap();
    std::fs::write(&pp, vec![0u8; CHUNK_SIZE * 3]).unwrap();
    write_record(out.path(), "pad.bin", &entry, &[0xFF], false, None);

    // 对账不 panic：填充位掩掉后 3 位全收 → Resume 且缺块位图全零
    let plans = intake::reconcile(out.path(), &man);
    match &plans[0] {
        FilePlan::Resume(miss) => assert!(miss.iter().all(|b| *b == 0), "3 位全收应无缺块"),
        other => panic!("expect Resume, got {other:?}"),
    }

    // 续传路径不得因下溢 panic / 回绕出巨大缺块数
    let w = PartWriter::open(out.path(), &entry, true).await.unwrap();
    assert_eq!(w.missing_count(), 0);
}
