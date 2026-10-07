//! 外观设置：深色模式管整个界面，读信窗格也跟着一起换色。
//!
//! 只读写本地界面偏好，不碰邮件数据，也不发任何网络请求。

import { useReaderTheme, type ReaderTheme } from "./readerTheme";

export default function AppearanceSettingsPanel() {
  const [theme, setTheme] = useReaderTheme();

  return (
    <section className="panel" aria-label="外观">
      <div className="panel-head">
        <h2>主题</h2>
        <p className="hint">
          整个界面和读信窗格一起换色；「跟随系统」跟着 Windows 的深浅色走。
        </p>
      </div>

      <label className="appearance-setting">
        <span>深色模式</span>
        <select
          aria-label="深色模式"
          value={theme}
          onChange={(event) => setTheme(event.target.value as ReaderTheme)}
        >
          <option value="auto">跟随系统</option>
          <option value="light">浅色</option>
          <option value="dark">深色</option>
        </select>
        <small className="hint">改完立即生效，不用保存。</small>
      </label>
    </section>
  );
}