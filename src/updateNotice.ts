//! 「已提醒过哪个版本」这个本机小状态的读写。
//!
//! 只放浏览器本地存储，不进设置文件、不参与账号同步；用户清了本地数据，最多
//! 同一个版本再提醒一次，不影响别处。

/** 已提醒过的新版本号存放位置。 */
export const UPDATE_NOTIFIED_VERSION_KEY = "ymail.update-notified-version.v1";

/** 读已提醒过的版本号；读不到（没有或存储不可用）就当没提醒过。 */
export function readNotifiedVersion(): string {
  try {
    return window.localStorage.getItem(UPDATE_NOTIFIED_VERSION_KEY) ?? "";
  } catch {
    return "";
  }
}

/** 这个版本还要不要提醒：跟已提醒过的不一样才提醒，同一个版本只提醒一次。 */
export function shouldNotifyVersion(version: string): boolean {
  if (!version) return false;
  return readNotifiedVersion() !== version;
}

/** 记住这个版本提醒过了；存储不可用就忽略，最多再提醒一次。 */
export function markVersionNotified(version: string): void {
  if (!version) return;
  try {
    window.localStorage.setItem(UPDATE_NOTIFIED_VERSION_KEY, version);
  } catch {
    // 存不下就算了，不拦使用。
  }
}
