//! 数据目录迁移：先规划、再复制、再校验。
//!
//! 这里故意不直接覆盖任何目标文件。只有全部校验通过后，调用方才把新目录写进
//! `settings.json`；设置写入前的任何失败都不会切换当前生效目录。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// 数据库文件名。首次启动判定与迁移计划都用它。
pub const DATABASE_NAME: &str = "ymail.db";
const DATABASE_SIDECARS: [&str; 2] = ["ymail.db-wal", "ymail.db-shm"];

/// 更改数据目录命令的返回结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeDataDirResult {
    /// 目标已有数据，需要界面先确认。
    pub needs_confirmation: bool,
    /// 给用户看的可读说明。
    pub message: String,
}

/// 一次迁移的完整计划。只有计划确认没有覆盖冲突后才会开始复制。
#[derive(Debug, Clone)]
pub struct MigrationPlan {
    new_dir: PathBuf,
    database_file: PathBuf,
    files: Vec<PlannedFile>,
}

impl MigrationPlan {
    /// 新目录的数据库文件路径。
    pub fn database_file(&self) -> &Path {
        &self.database_file
    }

    /// 新数据目录。
    pub fn new_dir(&self) -> &Path {
        &self.new_dir
    }
}

/// 规划阶段的结果。
#[derive(Debug, Clone)]
pub enum MigrationStart {
    /// 目标目录已有内容，等界面确认后再继续。
    NeedsConfirmation,
    /// 可以开始迁移。
    Ready(MigrationPlan),
}

#[derive(Debug, Clone)]
struct PlannedFile {
    source: PathBuf,
    destination: PathBuf,
    size: u64,
    database_part: bool,
}

/// 规划一次目录迁移。
///
/// `confirmed` 表示用户已经确认目标目录里的已有内容可以一起使用。即使确认了，
/// 只要会出现同名覆盖，也会拒绝继续，设置不会改变。
pub fn prepare_migration(
    active_dir: &Path,
    legacy_attachment_dir: Option<&Path>,
    new_dir: &Path,
    confirmed: bool,
) -> Result<MigrationStart, String> {
    if !new_dir.is_absolute() {
        return Err("新数据目录要填完整路径，例如 D:\\邮件".to_string());
    }
    fs::create_dir_all(new_dir)
        .map_err(|error| format!("新数据目录没法创建（{}）：{error}", new_dir.display()))?;

    let active_dir = canonicalize_existing(active_dir, "当前数据目录")?;
    let new_dir = canonicalize_existing(new_dir, "新数据目录")?;
    ensure_not_nested(&active_dir, &new_dir)?;
    ensure_writable(&new_dir)?;

    let target_has_entries = directory_has_entries(&new_dir)?;
    if target_has_entries && !confirmed {
        return Ok(MigrationStart::NeedsConfirmation);
    }
    if target_has_entries {
        for name in std::iter::once(DATABASE_NAME).chain(DATABASE_SIDECARS) {
            if new_dir.join(name).exists() {
                return Err(format!(
                    "目标目录里已有数据库文件「{name}」，为了避免覆盖已取消迁移；请换一个空目录"
                ));
            }
        }
    }

    let source_database = active_dir.join(DATABASE_NAME);
    if !source_database.is_file() {
        return Err(format!(
            "当前数据目录里找不到数据库文件「{DATABASE_NAME}」，不能安全迁移"
        ));
    }

    let mut files = Vec::new();
    collect_tree(&active_dir, &new_dir, true, &mut files)?;

    if let Some(legacy) = legacy_attachment_dir {
        if legacy.exists() {
            let legacy = canonicalize_existing(legacy, "旧附件目录")?;
            ensure_not_nested(&new_dir, &legacy)?;
            let default_downloads = active_dir.join("downloads");
            let skip_default = match canonicalize_existing(&default_downloads, "默认附件目录") {
                Ok(path) => path == legacy,
                Err(_) => false,
            };
            if !skip_default {
                let destination = new_dir.join("downloads");
                if destination.starts_with(&legacy) {
                    return Err("新数据目录的下载位置不能放在旧附件目录里面".to_string());
                }
                collect_tree(&legacy, &destination, false, &mut files)?;
            }
        }
    }

    reject_duplicate_destinations(&files)?;
    let total_bytes = total_size(&files)?;
    let available = fs2::available_space(&new_dir)
        .map_err(|error| format!("没法读取目标盘可用空间（{}）：{error}", new_dir.display()))?;
    if available < total_bytes {
        return Err(format!(
            "目标盘空间不够：迁移至少需要 {total_bytes} 字节，现在只剩 {available} 字节"
        ));
    }

    let database_file = new_dir.join(DATABASE_NAME);
    Ok(MigrationStart::Ready(MigrationPlan {
        new_dir,
        database_file,
        files,
    }))
}

/// 复制计划里除数据库之外的全部文件，并逐个核对大小。
///
/// 核对只拦「比规划时少」的情况。日志这类文件在迁移过程中仍在写入，复制到的
/// 字节数只会比规划时多，属于正常，不算失败。
///
/// 数据库由调用方通过 SQLite 自身的一致性快照写入，避免直接复制正在写入的
/// `ymail.db` / `-wal` / `-shm`。
pub fn copy_planned_files(plan: &MigrationPlan) -> Result<(), String> {
    for file in &plan.files {
        if file.database_part {
            continue;
        }
        if let Some(parent) = file.destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("创建目标目录失败（{}）：{error}", parent.display()))?;
        }
        fs::copy(&file.source, &file.destination).map_err(|error| {
            format!(
                "复制文件失败（{}）：{error}",
                file.source
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("文件")
            )
        })?;
        let actual = fs::metadata(&file.destination)
            .map_err(|error| format!("读取复制后文件失败（{}）：{error}", file.destination.display()))?
            .len();
        // 日志等文件在迁移期间还在追加，实际只会比规划时大；只有变小才说明没复制全。
        if actual < file.size {
            return Err(format!(
                "复制校验失败：{} 计划 {} 字节，实际只有 {} 字节；设置未改变",
                file.destination.display(),
                file.size,
                actual
            ));
        }
    }
    Ok(())
}

/// 打开迁移后的数据库并做一次 SQLite 官方完整性检查。
pub fn verify_database_file(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("迁移后的数据库文件不存在（{}）", path.display()));
    }
    let store = mail_core::Store::open(path).map_err(|error| format!("迁移后的数据库打不开：{error}"))?;
    store
        .integrity_check()
        .map_err(|error| format!("迁移后的数据库完整性检查没通过：{error}"))
}

fn collect_tree(
    source_root: &Path,
    destination_root: &Path,
    protect_database: bool,
    files: &mut Vec<PlannedFile>,
) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(source_root)
        .map_err(|error| format!("读取源目录失败（{}）：{error}", source_root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取源目录条目失败（{}）：{error}", source_root.display()))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let source = entry.path();
        let relative = source
            .strip_prefix(source_root)
            .map_err(|_| "源目录路径异常，不能继续迁移".to_string())?;
        let destination = destination_root.join(relative);
        let file_type = entry
            .file_type()
            .map_err(|error| format!("读取源文件类型失败（{}）：{error}", source.display()))?;

        if file_type.is_dir() {
            if destination.exists() && !destination.is_dir() {
                return Err(format!(
                    "目标位置已有同名文件，不能建目录：{}",
                    destination.display()
                ));
            }
            collect_tree(&source, &destination, protect_database, files)?;
            continue;
        }

        if !file_type.is_file() {
            return Err(format!(
                "源目录里有不能安全复制的特殊文件（{}），已取消迁移",
                source.display()
            ));
        }

        if destination.exists() {
            return Err(format!(
                "目标目录已有同名数据「{}」，为避免覆盖已取消迁移",
                destination.display()
            ));
        }

        let is_database_part =
            protect_database && relative.components().count() == 1 && is_database_name(&entry.file_name());
        let size = entry
            .metadata()
            .map_err(|error| format!("读取源文件大小失败（{}）：{error}", source.display()))?
            .len();
        files.push(PlannedFile {
            source,
            destination,
            size,
            database_part: is_database_part,
        });
    }
    Ok(())
}

fn reject_duplicate_destinations(files: &[PlannedFile]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for file in files {
        let key = file.destination.to_string_lossy().to_lowercase();
        if !seen.insert(key) {
            return Err(format!(
                "两个来源都会写到同一个位置（{}），为避免覆盖已取消迁移",
                file.destination.display()
            ));
        }
    }
    Ok(())
}

fn total_size(files: &[PlannedFile]) -> Result<u64, String> {
    files.iter().try_fold(0u64, |total, file| {
        total
            .checked_add(file.size)
            .ok_or_else(|| "待迁移文件总大小超出可处理范围".to_string())
    })
}

fn is_database_name(name: &std::ffi::OsStr) -> bool {
    let database = std::ffi::OsStr::new(DATABASE_NAME);
    name == database
        || DATABASE_SIDECARS
            .iter()
            .any(|value| name == std::ffi::OsStr::new(value))
}

fn directory_has_entries(path: &Path) -> Result<bool, String> {
    let mut entries =
        fs::read_dir(path).map_err(|error| format!("读取目标目录失败（{}）：{error}", path.display()))?;
    Ok(entries.next().is_some())
}

fn ensure_writable(path: &Path) -> Result<(), String> {
    let probe = path.join(format!(".ymail-write-test-{}", std::process::id()));
    match fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(error) => Err(format!("目标目录不可写（{}）：{error}", path.display())),
    }
}

fn canonicalize_existing(path: &Path, label: &str) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format!("{label}没法访问（{}）：{error}", path.display()))
}

fn ensure_not_nested(left: &Path, right: &Path) -> Result<(), String> {
    if left == right {
        return Err("新数据目录不能和当前数据目录是同一个目录".to_string());
    }
    if right.starts_with(left) {
        return Err("新数据目录不能放在当前数据目录里面".to_string());
    }
    if left.starts_with(right) {
        return Err("新数据目录不能是当前数据目录的上一级".to_string());
    }
    Ok(())
}

// ============================ 旧数据目录清理 ============================

/// 清理旧目录时允许删除的固定目录名（相对旧目录）。
const CLEANUP_DIRS: [&str; 3] = ["logs", "downloads", "compose-images"];
/// 清理旧目录时按前缀匹配的备份目录名。
const CLEANUP_BACKUP_PREFIX: &str = "backup-";

/// 一次清理的结果：删了什么、跳过了什么，调用方拿去写日志。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CleanupOutcome {
    /// 已经删掉的目标（相对旧目录的名字）。
    pub removed: Vec<String>,
    /// 没删掉或主动跳过的目标与原因。
    pub skipped: Vec<String>,
}

/// 清理旧数据目录里属于本应用的文件与目录。
///
/// 只动清单里的项：数据库三件套、`logs`、`downloads`、`compose-images`、
/// `backup-*` 目录。**不删 `settings.json`，也不删清单以外的任何东西。**
/// 安全闸任一条不满足时一个都不删，原因记在 `skipped` 里。
pub fn cleanup_old_data_dir(old_dir: &Path, active_dir: &Path) -> CleanupOutcome {
    let mut outcome = CleanupOutcome::default();

    if let Some(reason) = cleanup_blocked(old_dir, active_dir) {
        outcome.skipped.push(reason);
        return outcome;
    }

    let old_dir = fs::canonicalize(old_dir).unwrap_or_else(|_| old_dir.to_path_buf());

    for name in std::iter::once(DATABASE_NAME).chain(DATABASE_SIDECARS) {
        remove_cleanup_target(&old_dir, Path::new(name), &mut outcome);
    }
    for name in CLEANUP_DIRS {
        remove_cleanup_target(&old_dir, Path::new(name), &mut outcome);
    }

    // `backup-*` 按前缀匹配；名字来自目录遍历，不会跳出旧目录。
    match fs::read_dir(&old_dir) {
        Ok(entries) => {
            let mut names: Vec<String> = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(CLEANUP_BACKUP_PREFIX) && entry.path().is_dir() {
                    names.push(name);
                }
            }
            names.sort();
            for name in names {
                remove_cleanup_target(&old_dir, Path::new(&name), &mut outcome);
            }
        }
        Err(error) => outcome
            .skipped
            .push(format!("旧目录读不开，备份目录没检查：{error}")),
    }

    outcome
}

/// 删掉旧目录下的一个目标；不存在就跳过，删不掉只记原因、不中断其余项。
fn remove_cleanup_target(old_dir: &Path, relative: &Path, outcome: &mut CleanupOutcome) {
    let target = old_dir.join(relative);
    let name = relative.to_string_lossy().to_string();
    if !target.exists() {
        return;
    }
    let result = if target.is_dir() {
        fs::remove_dir_all(&target)
    } else {
        fs::remove_file(&target)
    };
    match result {
        Ok(()) => outcome.removed.push(name),
        Err(error) => outcome.skipped.push(format!("{name} 没删掉：{error}")),
    }
}

/// 清理前的安全闸：任一条不满足就返回原因，调用方一个都不删。
fn cleanup_blocked(old_dir: &Path, active_dir: &Path) -> Option<String> {
    if !old_dir.exists() {
        return Some("旧数据目录不存在，跳过清理".to_string());
    }
    if is_filesystem_root(old_dir) {
        return Some(format!(
            "旧数据目录是盘符根目录（{}），为避免误删跳过清理",
            old_dir.display()
        ));
    }
    let old = fs::canonicalize(old_dir).unwrap_or_else(|_| old_dir.to_path_buf());
    let active = fs::canonicalize(active_dir).unwrap_or_else(|_| active_dir.to_path_buf());
    if old == active {
        return Some("待清理目录就是当前生效目录，跳过清理".to_string());
    }
    if active.starts_with(&old) {
        return Some("当前生效目录在待清理目录里面，跳过清理".to_string());
    }
    None
}

/// 是不是盘符根目录 / 文件系统根（例如 `E:\`、`/`）。
fn is_filesystem_root(path: &Path) -> bool {
    use std::path::Component;
    path.has_root()
        && path
            .components()
            .all(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_old_data_dir, copy_planned_files, is_filesystem_root, prepare_migration,
        verify_database_file, MigrationStart,
    };

    fn write(path: &std::path::Path, content: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("建父目录");
        }
        std::fs::write(path, content).expect("写测试文件");
    }

    #[test]
    fn plans_and_copies_all_non_database_files() {
        let root = tempfile::tempdir().expect("临时目录");
        let old = root.path().join("old");
        let new = root.path().join("new");
        write(&old.join("ymail.db"), b"source-db");
        write(&old.join("logs").join("app.log"), b"log-data");
        write(&old.join("downloads").join("file.bin"), b"attachment");

        let expected = match prepare_migration(&old, None, &new, false).expect("规划迁移") {
            MigrationStart::Ready(plan) => plan,
            MigrationStart::NeedsConfirmation => panic!("空目标目录不该要求确认"),
        };
        write(expected.database_file(), b"backup-db");
        copy_planned_files(&expected).expect("复制普通文件");

        assert_eq!(
            std::fs::read(new.join("logs").join("app.log")).unwrap(),
            b"log-data"
        );
        assert_eq!(
            std::fs::read(new.join("downloads").join("file.bin")).unwrap(),
            b"attachment"
        );
        assert_eq!(std::fs::read(new.join("ymail.db")).unwrap(), b"backup-db");
        assert!(old.join("ymail.db").exists(), "旧目录必须保留");
    }

    #[test]
    fn existing_target_requires_confirmation_and_never_overwrites() {
        let root = tempfile::tempdir().expect("临时目录");
        let old = root.path().join("old");
        let new = root.path().join("new");
        write(&old.join("ymail.db"), b"source-db");
        write(&new.join("ymail.db"), b"existing-db");

        assert!(matches!(
            prepare_migration(&old, None, &new, false).expect("先返回确认"),
            MigrationStart::NeedsConfirmation
        ));
        let error = prepare_migration(&old, None, &new, true).expect_err("确认后也不能覆盖");
        assert!(error.contains("数据库文件"), "错误应说明数据库同名：{error}");
    }

    #[test]
    fn growing_file_is_allowed_and_copied_fully() {
        let root = tempfile::tempdir().expect("临时目录");
        let old = root.path().join("old");
        let new = root.path().join("new");
        write(&old.join("ymail.db"), b"source-db");
        write(&old.join("logs").join("app.log"), b"small");

        let plan = match prepare_migration(&old, None, &new, false).expect("规划迁移") {
            MigrationStart::Ready(plan) => plan,
            MigrationStart::NeedsConfirmation => panic!("空目标目录不该要求确认"),
        };
        // 模拟日志在迁移期间继续追加：规划时 5 字节，复制时已长到 13 字节。
        let grown = b"a-larger-file";
        write(&old.join("logs").join("app.log"), grown);
        copy_planned_files(&plan).expect("日志变大属于正常，不该失败");
        assert_eq!(
            std::fs::read(new.join("logs").join("app.log")).unwrap(),
            grown,
            "复制结果要和复制那一刻的源文件一致"
        );
    }

    #[test]
    fn shrinking_file_aborts_without_switching_settings() {
        let root = tempfile::tempdir().expect("临时目录");
        let old = root.path().join("old");
        let new = root.path().join("new");
        write(&old.join("ymail.db"), b"source-db");
        write(&old.join("logs").join("app.log"), b"small");

        let plan = match prepare_migration(&old, None, &new, false).expect("规划迁移") {
            MigrationStart::Ready(plan) => plan,
            MigrationStart::NeedsConfirmation => panic!("空目标目录不该要求确认"),
        };
        // 复制到的内容比规划时少，说明没复制全，必须失败。
        write(&old.join("logs").join("app.log"), b"t");
        let error = copy_planned_files(&plan).expect_err("文件变小应失败");
        assert!(error.contains("复制校验失败"), "错误应可读：{error}");
    }

    #[test]
    fn verifies_a_valid_sqlite_database() {
        let root = tempfile::tempdir().expect("临时目录");
        let db = root.path().join("ymail.db");
        let store = mail_core::Store::open(&db).expect("建库");
        drop(store);
        verify_database_file(&db).expect("完整性检查通过");
    }

    #[test]
    fn cleanup_removes_only_listed_items_and_keeps_settings() {
        let root = tempfile::tempdir().expect("临时目录");
        let old = root.path().join("old");
        let active = root.path().join("new");
        std::fs::create_dir_all(&active).expect("建新目录");
        write(&old.join("ymail.db"), b"db");
        write(&old.join("ymail.db-wal"), b"wal");
        write(&old.join("ymail.db-shm"), b"shm");
        write(&old.join("logs").join("ymail.log.1"), b"log");
        write(&old.join("downloads").join("a.bin"), b"file");
        write(&old.join("compose-images").join("b.png"), b"png");
        write(&old.join("backup-2026").join("ymail.db"), b"old");
        write(&old.join("settings.json"), b"{}");
        write(&old.join("keep-me.txt"), b"keep");

        let outcome = cleanup_old_data_dir(&old, &active);
        assert!(outcome.skipped.is_empty(), "不该有跳过：{:?}", outcome.skipped);
        assert!(!old.join("ymail.db").exists());
        assert!(!old.join("ymail.db-wal").exists());
        assert!(!old.join("logs").exists());
        assert!(!old.join("downloads").exists());
        assert!(!old.join("compose-images").exists());
        assert!(!old.join("backup-2026").exists());
        assert!(old.join("settings.json").exists(), "设置文件必须保留");
        assert!(old.join("keep-me.txt").exists(), "清单外内容必须保留");
    }

    #[test]
    fn cleanup_safety_gate_blocks_current_root_and_nested_active_dir() {
        let root = tempfile::tempdir().expect("临时目录");
        let dir = root.path().join("data");
        std::fs::create_dir_all(dir.join("inner")).expect("建目录");
        write(&dir.join("ymail.db"), b"db");

        // 待清理目录就是当前生效目录：不许删。
        let outcome = cleanup_old_data_dir(&dir, &dir);
        assert!(dir.join("ymail.db").exists());
        assert!(!outcome.skipped.is_empty(), "要说明为什么跳过");

        // 当前生效目录在待清理目录里面：不许删。
        let outcome = cleanup_old_data_dir(&dir, &dir.join("inner"));
        assert!(dir.join("ymail.db").exists());
        assert!(!outcome.skipped.is_empty(), "要说明为什么跳过");

        // 盘符根目录认得出；普通目录不能误判成根。
        assert!(is_filesystem_root(std::path::Path::new("E:\\")));
        assert!(!is_filesystem_root(&dir));
    }

    #[test]
    fn cleanup_missing_dir_is_skipped() {
        let root = tempfile::tempdir().expect("临时目录");
        let gone = root.path().join("gone");
        let active = root.path().join("active");
        std::fs::create_dir_all(&active).expect("建新目录");
        let outcome = cleanup_old_data_dir(&gone, &active);
        assert!(outcome.removed.is_empty());
        assert!(!outcome.skipped.is_empty(), "不存在的目录要说明为什么跳过");
    }
}
