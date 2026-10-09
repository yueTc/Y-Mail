//! 单个文件夹的同步：先建快照，再拉增量，最后分批补齐历史。
//!
//! 断点口径：`folder.synced_min_uid` 是「后台补齐已达到的最小 UID」，
//! 增量前沿是本地已有的最大 UID。UIDVALIDITY 变了就清空该文件夹重来，
//! 避免把两代 UID 混在一起。

use std::sync::Arc;

use mail_domain::dates::{format_imap_date, format_iso8601_utc, parse_mail_date, unix_now};
use mail_domain::{thread_key, FolderKind, HistoryRange};
use mail_imap::{Address, FolderInfo, ImapClient, MessageMeta};
use mail_mime::decode_encoded_words;
use mail_store::{NewFolder, NewMessage, StoredFolder};

use super::state::SyncState;
use super::worker::{lock_store, refresh_progress, Failure, WorkerContext, FETCH_BATCH};

/// 一次唤醒里最多补几批历史，避免长时间占着连接不处理新邮件。
const BACKFILL_BATCHES_PER_CYCLE: usize = 8;

/// 把服务器返回的文件夹列表对齐到本地：认领旧乱码行、合并重复、清掉空的残留。
pub(super) fn align_folders(ctx: &Arc<WorkerContext>, folders: &[FolderInfo]) -> Result<(), Failure> {
    let prepared: Vec<NewFolder> = folders
        .iter()
        .map(|info| NewFolder {
            full_path: info.full_path.clone(),
            server_path: info.server_path.clone(),
            delimiter: info.delimiter.clone(),
            kind: FolderKind::classify(&info.full_path, &info.attributes),
        })
        .collect();
    let store = lock_store(&ctx.store);
    store
        .align_folders(ctx.account_id, &prepared)
        .map_err(Failure::store)
}

/// 同步一个文件夹：SELECT → 处理 UIDVALIDITY → 快照 / 增量 / 补齐 → 收尾。
pub(super) async fn sync_folder(
    ctx: &Arc<WorkerContext>,
    client: &mut ImapClient,
    info: &FolderInfo,
) -> Result<(), Failure> {
    if ctx.cancel.is_cancelled() {
        return Ok(());
    }
    let kind = FolderKind::classify(&info.full_path, &info.attributes);
    let folder_id = {
        let store = lock_store(&ctx.store);
        store
            .upsert_folder_mapped(
                ctx.account_id,
                &info.full_path,
                &info.server_path,
                &info.delimiter,
                kind,
            )
            .map_err(Failure::store)?
    };

    let status = client
        .select(&info.server_path)
        .await
        .map_err(Failure::connection)?;

    // 先看库里旧的 UIDVALIDITY；变了就清空该文件夹。
    let previous = load_folder(ctx, folder_id)?;
    let uid_changed = matches!(
        (&previous, status.uidvalidity),
        (Some(folder), Some(new))
            if folder.uidvalidity.is_some() && folder.uidvalidity != Some(new)
    );
    if uid_changed {
        tracing::info!(
            account = ctx.account_id,
            folder = %info.full_path,
            "UIDVALIDITY 变化，清空该文件夹重新同步"
        );
        let store = lock_store(&ctx.store);
        store.delete_folder_messages(folder_id).map_err(Failure::store)?;
        store
            .set_folder_synced_min_uid(folder_id, None)
            .map_err(Failure::store)?;
    }
    {
        let store = lock_store(&ctx.store);
        store
            .update_folder_select(
                folder_id,
                status.uidvalidity,
                status.uidnext,
                i64::from(status.unseen),
            )
            .map_err(Failure::store)?;
    }

    let range = history_range(ctx);
    let has_local = {
        let store = lock_store(&ctx.store);
        store
            .max_message_uid(folder_id)
            .map_err(Failure::store)?
            .is_some()
    };
    if !has_local {
        snapshot(ctx, client, folder_id).await?;
    }
    incremental(ctx, client, folder_id).await?;
    backfill(ctx, client, folder_id, range).await?;

    refresh_attachment_flags(ctx, client, folder_id).await?;

    {
        let store = lock_store(&ctx.store);
        store.touch_folder_synced_at(folder_id).map_err(Failure::store)?;
    }
    Ok(())
}

/// 一次性补齐已有邮件的附件标记。
///
/// 列表抓取早先不请求 BODYSTRUCTURE，老库里 `has_attachments` 全是 0。升级后把该
/// 文件夹已入库的 UID 重抓一遍元数据（写入路径只刷新附件标记，不动本地状态），
/// 跑完在设置项里记一笔，之后不再重复。
async fn refresh_attachment_flags(
    ctx: &Arc<WorkerContext>,
    client: &mut ImapClient,
    folder_id: i64,
) -> Result<(), Failure> {
    let key = format!("sync.attachment_scan.v1.{folder_id}");
    let done = {
        let store = lock_store(&ctx.store);
        store.get_setting(&key).map_err(Failure::store)?.is_some()
    };
    if done {
        return Ok(());
    }
    let uids = {
        let store = lock_store(&ctx.store);
        store.list_message_uids(folder_id).map_err(Failure::store)?
    };
    if uids.is_empty() {
        let store = lock_store(&ctx.store);
        store.set_setting(&key, "empty").map_err(Failure::store)?;
        return Ok(());
    }
    // 这里不再单独改状态：账号级进度只由 refresh_progress 维护，
    // 免得这次一次性的老库附件扫描把「共多少封」冲掉。
    for chunk in uids.chunks(ctx.config.backfill_batch) {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        let metas = fetch_all(ctx, client, chunk).await?;
        insert_metas(ctx, folder_id, metas)?;
    }
    let store = lock_store(&ctx.store);
    store.set_setting(&key, "done").map_err(Failure::store)?;
    Ok(())
}

/// 首次快照：近若干天里取最新的若干封，并把断点落在这一批的最小 UID 上。
async fn snapshot(ctx: &Arc<WorkerContext>, client: &mut ImapClient, folder_id: i64) -> Result<(), Failure> {
    let date = format_imap_date(ctx.config.snapshot_days);
    let mut uids = client
        .uid_search_since(&date)
        .await
        .map_err(Failure::connection)?;
    if uids.len() > ctx.config.snapshot_limit {
        let start = uids.len() - ctx.config.snapshot_limit;
        uids = uids.split_off(start);
    }
    if uids.is_empty() {
        return Ok(());
    }
    refresh_progress(ctx, SyncState::Syncing, false);
    let metas = fetch_all(ctx, client, &uids).await?;
    let capped = insert_metas(ctx, folder_id, metas)?;
    refresh_progress(ctx, SyncState::Syncing, capped);
    let min_uid = uids.first().copied();
    let store = lock_store(&ctx.store);
    store
        .set_folder_synced_min_uid(folder_id, min_uid)
        .map_err(Failure::store)?;
    Ok(())
}

/// 增量：把本地最大 UID 之后的新邮件补齐。
async fn incremental(
    ctx: &Arc<WorkerContext>,
    client: &mut ImapClient,
    folder_id: i64,
) -> Result<(), Failure> {
    let max_uid = {
        let store = lock_store(&ctx.store);
        store.max_message_uid(folder_id).map_err(Failure::store)?
    };
    let from_uid = max_uid.map_or(1, |uid| uid.saturating_add(1));
    let uids = client
        .uid_search_after(from_uid)
        .await
        .map_err(Failure::connection)?;
    if uids.is_empty() {
        return Ok(());
    }
    refresh_progress(ctx, SyncState::Syncing, false);
    let metas = fetch_all(ctx, client, &uids).await?;
    let capped = insert_metas(ctx, folder_id, metas)?;
    refresh_progress(ctx, SyncState::Syncing, capped);
    Ok(())
}

/// 历史补齐：从断点往更早拉，直到达到范围下界或没有更早的邮件。
async fn backfill(
    ctx: &Arc<WorkerContext>,
    client: &mut ImapClient,
    folder_id: i64,
    range: HistoryRange,
) -> Result<(), Failure> {
    let floor = backfill_floor(ctx, client, range).await?;
    if floor == 0 {
        return Ok(());
    }
    for _ in 0..BACKFILL_BATCHES_PER_CYCLE {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        let cursor = {
            let store = lock_store(&ctx.store);
            store
                .get_folder_by_id(folder_id)
                .map_err(Failure::store)?
                .and_then(|folder| folder.synced_min_uid)
        };
        let cursor = match cursor {
            Some(value) if value > floor => value,
            _ => break,
        };
        let mut uids = client
            .uid_search_older_than(cursor)
            .await
            .map_err(Failure::connection)?;
        uids.retain(|uid| *uid >= floor);
        if uids.is_empty() {
            break;
        }
        if uids.len() > ctx.config.backfill_batch {
            let start = uids.len() - ctx.config.backfill_batch;
            uids = uids.split_off(start);
        }
        refresh_progress(ctx, SyncState::Backfilling, false);
        let metas = fetch_all(ctx, client, &uids).await?;
        let capped = insert_metas(ctx, folder_id, metas)?;
        refresh_progress(ctx, SyncState::Backfilling, capped);
        let next_cursor = uids.first().copied().unwrap_or(floor);
        let store = lock_store(&ctx.store);
        store
            .set_folder_synced_min_uid(folder_id, Some(next_cursor))
            .map_err(Failure::store)?;
    }
    Ok(())
}

/// 补齐下界：范围内最早的 UID；范围是「全部」时下界为 1。
async fn backfill_floor(
    _ctx: &Arc<WorkerContext>,
    client: &mut ImapClient,
    range: HistoryRange,
) -> Result<u32, Failure> {
    match range.cutoff_days() {
        None => Ok(1),
        Some(days) => {
            let date = format_imap_date(days);
            let uids = client
                .uid_search_since(&date)
                .await
                .map_err(Failure::connection)?;
            Ok(uids.first().copied().unwrap_or(0))
        }
    }
}

/// 把 UID 分批抓成元数据（单条命令太大容易把应答撑爆）。
async fn fetch_all(
    ctx: &Arc<WorkerContext>,
    client: &mut ImapClient,
    uids: &[u32],
) -> Result<Vec<MessageMeta>, Failure> {
    let mut metas = Vec::with_capacity(uids.len());
    for chunk in uids.chunks(FETCH_BATCH) {
        if ctx.cancel.is_cancelled() {
            break;
        }
        let mut part = client.fetch_metadata(chunk).await.map_err(Failure::connection)?;
        metas.append(&mut part);
    }
    Ok(metas)
}

/// 组装并写入一批元数据。
///
/// 返回是否因为达到账号容量上限而没写；撞上限也照常让断点前进，不反复重试同一批。
fn insert_metas(ctx: &Arc<WorkerContext>, folder_id: i64, metas: Vec<MessageMeta>) -> Result<bool, Failure> {
    if metas.is_empty() {
        return Ok(false);
    }
    // 先把本次邮件的收发件人收集出来，写入正文前顺手登记地址簿。
    let mut contacts: Vec<(String, String)> = Vec::new();
    for meta in &metas {
        for addr in meta
            .envelope
            .from
            .iter()
            .chain(meta.envelope.to.iter())
            .chain(meta.envelope.cc.iter())
        {
            if !addr.address.trim().is_empty() {
                contacts.push((decode_encoded_words(&addr.name), addr.address.clone()));
            }
        }
    }
    let messages: Vec<NewMessage> = metas
        .into_iter()
        .map(|meta| to_new_message(ctx.account_id, folder_id, meta))
        .collect();
    let store = lock_store(&ctx.store);
    let count = store
        .count_account_messages(ctx.account_id)
        .map_err(Failure::store)?;
    let bytes = store
        .account_message_bytes(ctx.account_id)
        .map_err(Failure::store)?;
    if count >= ctx.config.max_messages_per_account || bytes >= ctx.config.max_bytes_per_account {
        tracing::warn!(account = ctx.account_id, "本地已达账号容量上限，暂停写入新邮件");
        return Ok(true);
    }
    let inserted = store.insert_messages(&messages).map_err(Failure::store)?;
    if inserted > 0 {
        store.upsert_contacts(&contacts).map_err(Failure::store)?;
    }
    Ok(false)
}

/// 一条服务器元数据 → 一行本地邮件。
fn to_new_message(account_id: i64, folder_id: i64, meta: MessageMeta) -> NewMessage {
    let envelope = meta.envelope;
    let from = envelope.from.first();
    let date_utc = parse_mail_date(&envelope.date)
        .or_else(|| parse_mail_date(&meta.internal_date))
        .unwrap_or_else(|| format_iso8601_utc(unix_now()));
    let upper: Vec<String> = meta.flags.iter().map(|flag| flag.to_ascii_uppercase()).collect();
    let has = |name: &str| upper.iter().any(|flag| flag == name);
    // 主题与显示名在 IMAP ENVELOPE 里常是 RFC 2047 编码字，先解成正常文字再入库，
    // 免得列表和读信页显示出一串 `=?UTF-8?B?...?=`。
    let subject = decode_encoded_words(&envelope.subject);
    let from_name = from
        .map(|addr| decode_encoded_words(&addr.name))
        .unwrap_or_default();
    NewMessage {
        account_id,
        folder_id,
        uid: meta.uid,
        message_id_header: envelope.message_id.clone(),
        thread_key: thread_key(&subject, Some(&envelope.message_id)),
        subject,
        from_name,
        from_addr: from.map(|addr| addr.address.clone()).unwrap_or_default(),
        to_json: addresses_json(&envelope.to),
        cc_json: addresses_json(&envelope.cc),
        date_utc,
        size: meta.size,
        has_attachments: meta.has_attachments,
        is_read: has("\\SEEN"),
        is_flagged: has("\\FLAGGED"),
        is_answered: has("\\ANSWERED"),
        is_draft: has("\\DRAFT"),
    }
}

/// 地址列表序列化成 JSON 数组（只存显示名与邮箱）。
fn addresses_json(list: &[Address]) -> String {
    let items: Vec<serde_json::Value> = list
        .iter()
        .map(|addr| {
            serde_json::json!({
                "name": decode_encoded_words(&addr.name),
                "address": addr.address,
            })
        })
        .collect();
    serde_json::Value::Array(items).to_string()
}

/// 读设置项里的补齐范围，缺省用引擎参数。
fn history_range(ctx: &Arc<WorkerContext>) -> HistoryRange {
    let store = lock_store(&ctx.store);
    let value = store.get_setting("sync.history_range").ok().flatten();
    let days = store
        .get_setting("sync.history_days")
        .ok()
        .flatten()
        .and_then(|raw| raw.parse::<u32>().ok());
    value
        .as_deref()
        .and_then(|raw| HistoryRange::parse(raw, days))
        .unwrap_or(ctx.config.history_range)
}

/// 按主键读一行文件夹。
fn load_folder(ctx: &Arc<WorkerContext>, folder_id: i64) -> Result<Option<StoredFolder>, Failure> {
    let store = lock_store(&ctx.store);
    store.get_folder_by_id(folder_id).map_err(Failure::store)
}

#[cfg(test)]
mod tests {
    use super::to_new_message;
    use mail_imap::{Address, Envelope, MessageMeta};

    fn meta(subject: &str, from_name: &str) -> MessageMeta {
        MessageMeta {
            uid: 1,
            flags: Vec::new(),
            internal_date: "2026-10-05T01:00:00Z".to_string(),
            size: 100,
            has_attachments: false,
            envelope: Envelope {
                date: "2026-10-05T09:00:00+0800".to_string(),
                subject: subject.to_string(),
                from: vec![Address {
                    name: from_name.to_string(),
                    address: "alice@example.com".to_string(),
                }],
                to: vec![Address {
                    name: "=?UTF-8?Q?Bob_Lee?=".to_string(),
                    address: "bob@example.com".to_string(),
                }],
                cc: Vec::new(),
                in_reply_to: String::new(),
                message_id: "<m1@example.com>".to_string(),
            },
        }
    }

    #[test]
    fn 入库前会把编码字主题与显示名解出来() {
        let message = to_new_message(
            1,
            2,
            meta("=?UTF-8?B?5L2g5aW9?= 报告", "=?ISO-8859-1?Q?Olle_J=E4rnefors?="),
        );
        assert_eq!(message.subject, "你好 报告");
        assert_eq!(message.from_name, "Olle Järnefors");
        assert_eq!(message.thread_key, "你好 报告");
        assert!(message.to_json.contains("Bob Lee"), "{}", message.to_json);
    }

    #[test]
    fn 普通主题入库保持不变() {
        let message = to_new_message(1, 2, meta("季度报价", "李四"));
        assert_eq!(message.subject, "季度报价");
        assert_eq!(message.from_name, "李四");
    }
}
