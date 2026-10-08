//! 外观设置：深色模式管整个界面，读信窗格也跟着一起换色。
//!
//! 界面语言也放在这里：只读写本地偏好，不碰邮件数据，也不发任何网络请求。

import { t, useLanguage, type Language } from "./i18n";
import { useReaderTheme, type ReaderTheme } from "./readerTheme";

export default function AppearanceSettingsPanel() {
  const [theme, setTheme] = useReaderTheme();
  const [language, setLanguage] = useLanguage();

  return (
    <section className="panel" aria-label={t("外观")}>
      <div className="panel-head">
        <h2>{t("主题")}</h2>
      </div>

      <label className="appearance-setting">
        <span>{t("深色模式")}</span>
        <select
          aria-label={t("深色模式")}
          value={theme}
          onChange={(event) => setTheme(event.target.value as ReaderTheme)}
        >
          <option value="auto">{t("跟随系统")}</option>
          <option value="light">{t("浅色")}</option>
          <option value="dark">{t("深色")}</option>
        </select>
      </label>

      <label className="appearance-setting">
        <span>{t("界面语言")}</span>
        <select
          aria-label={t("界面语言")}
          value={language}
          onChange={(event) => setLanguage(event.target.value as Language)}
        >
          <option value="zh">中文</option>
          <option value="en">English</option>
        </select>
      </label>
    </section>
  );
}