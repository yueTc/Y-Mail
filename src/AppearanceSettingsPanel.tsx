//! 外观设置：深色模式管整个界面，读信窗格也跟着一起换色。
//!
//! 界面语言、页面缩放、字体大小、字体样式也放在这里：都只读写本地偏好，
//! 不碰邮件数据，也不发任何网络请求。

import { t, useLanguage, type Language } from "./i18n";
import { useReaderTheme, type ReaderTheme } from "./readerTheme";
import {
  UI_FONT_FAMILIES,
  UI_FONT_FAMILY_LABELS,
  UI_FONT_LEVELS,
  UI_ZOOM_LEVELS,
  useUiScale,
  type UiFontFamily,
} from "./uiScale";

export default function AppearanceSettingsPanel() {
  const [theme, setTheme] = useReaderTheme();
  const [language, setLanguage] = useLanguage();
  const { zoom, font, fontFamily, setZoom, setFont, setFontFamily } = useUiScale();

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

      <label className="appearance-setting">
        <span>{t("页面缩放")}</span>
        <select
          aria-label={t("页面缩放")}
          value={zoom}
          onChange={(event) => setZoom(Number(event.target.value))}
        >
          {UI_ZOOM_LEVELS.map((level) => (
            <option key={level} value={level}>
              {`${Math.round(level * 100)}%`}
            </option>
          ))}
        </select>
      </label>

      <label className="appearance-setting">
        <span>{t("字体大小")}</span>
        <select
          aria-label={t("字体大小")}
          value={font}
          onChange={(event) => setFont(Number(event.target.value))}
        >
          {UI_FONT_LEVELS.map((size) => (
            <option key={size} value={size}>
              {`${size} px`}
            </option>
          ))}
        </select>
      </label>

      <label className="appearance-setting">
        <span>{t("字体样式")}</span>
        <select
          aria-label={t("字体样式")}
          value={fontFamily}
          onChange={(event) => setFontFamily(event.target.value as UiFontFamily)}
        >
          {UI_FONT_FAMILIES.map((family) => (
            <option key={family} value={family}>
              {t(UI_FONT_FAMILY_LABELS[family])}
            </option>
          ))}
        </select>
      </label>
    </section>
  );
}